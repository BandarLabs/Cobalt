//! Runtime-owned MP3 playback through Kobo's AOSP A2DP HAL.
//!
//! Bluetooth connection ownership stays with the firmware's `btservice`.
//! Once an audio device is connected it exposes the standard AOSP
//! `audio_a2dp_hw` control and data sockets. The player sends CHECK_READY,
//! START and STOP on the control socket and paced, 48 kHz stereo S16LE PCM
//! on the data socket. No Nickel process or audio command-line utility is
//! required.

use kobo_doc::zip::Archive;
use kobo_protocol::{AudioPlaybackState, DeviceError, DeviceResult};
use minimp3::{Decoder, Error as Mp3Error};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

pub const CONTROL_SOCKET: &str = "/tmp/audio.a2dp_ctrl";
pub const DATA_SOCKET: &str = "/tmp/audio.a2dp_data";

const COMMAND_CHECK_READY: u8 = 0x01;
const COMMAND_START: u8 = 0x02;
const COMMAND_STOP: u8 = 0x03;
const ACK_SUCCESS: u8 = 0x00;
/// What the firmware's Bluetooth encoder consumes. Found empirically on the
/// Clara BW: a 44.1 kHz sine over the data socket stutters and plays sharp,
/// the same tone generated at 48 kHz plays clean and true, so the encoder
/// reads the stream as 48 kHz no matter what it is fed.
const TARGET_RATE: u64 = 48_000;
const TARGET_RATE_I64: i64 = 48_000;
const CHUNK_FRAMES: usize = 2_400;
const LEAD_IN_FRAMES: usize = 24_000;
/// How much audio one chunk carries, and therefore the write cadence.
const WRITE_PERIOD: Duration = Duration::from_millis(50);
/// A schedule this far behind is a stall, not jitter, and is restarted
/// rather than caught up with a burst of writes.
const RESYNC_LIMIT: Duration = Duration::from_millis(500);
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const DATA_CONNECT_TIMEOUT: Duration = Duration::from_millis(750);
const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_TRACK_BYTES: usize = 8 * 1024 * 1024;
const MAX_TRACK_FRAMES: usize = 8_000_000;
const MAX_TRACK_FRAMES_U64: u64 = 8_000_000;
const _: () = assert!(MAX_TRACK_FRAMES * 4 <= 32 * 1024 * 1024);
const FETCH_CHUNK: u32 = 512 * 1024;

const BACKEND_MARKERS: [&str; 8] = [
    // Clara BW 4.45.23697. Unlike many Android trees, Kobo installs the HAL
    // directly in /usr/lib rather than beneath an hw/ directory.
    "/usr/lib/libaudio.a2dp.default.so",
    "/usr/lib/hw/audio.a2dp.default.so",
    "/usr/local/Kobo/lib/hw/audio.a2dp.default.so",
    "/usr/local/Kobo/lib/audio.a2dp.default.so",
    "/usr/local/Kobo/audio.a2dp.default.so",
    "/usr/local/Kobo/btservice",
    "/usr/local/Kobo/bluetooth/btservice",
    CONTROL_SOCKET,
];

/// Bounded range fetch used by stream sources. The runtime injects its own TLS
/// transport, keeping credentials and sockets out of SDK applications.
pub type StreamFetcher = Arc<dyn Fn(&str, u32, u32) -> Result<Vec<u8>, DeviceError> + Send + Sync>;

#[derive(Clone, Debug)]
pub enum Source {
    File(PathBuf),
    Stream(String),
}

#[derive(Clone, Copy, Debug)]
struct State {
    playback: AudioPlaybackState,
    position_ms: u32,
    duration_ms: u32,
    volume: u8,
    error: Option<DeviceError>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            playback: AudioPlaybackState::Idle,
            position_ms: 0,
            duration_ms: 0,
            volume: 70,
            error: None,
        }
    }
}

enum Command {
    Load(Source),
    Play { restart: bool },
    Pause,
    Seek(u32),
    Stop,
    Volume(u8),
    Shutdown,
}

/// One session-wide audio transport. Commands are non-blocking; decoding,
/// range fetching and paced socket writes happen on its worker thread.
pub struct Audio {
    sender: mpsc::Sender<Command>,
    state: Arc<Mutex<State>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Audio {
    /// Opens the backend when the firmware contains the A2DP HAL. The live
    /// sockets are deliberately not required here: they appear only after an
    /// audio device connects, while applications must be able to discover the
    /// capability before asking the user to pair one.
    #[must_use]
    pub fn open(fetcher: Option<StreamFetcher>) -> Option<Self> {
        if !BACKEND_MARKERS
            .iter()
            .any(|marker| Path::new(marker).exists())
        {
            return None;
        }
        Some(Self::spawn(fetcher))
    }

    #[cfg(test)]
    fn simulated() -> Self {
        Self::spawn(None)
    }

    fn spawn(fetcher: Option<StreamFetcher>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let state = Arc::new(Mutex::new(State::default()));
        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || run(&receiver, &worker_state, fetcher.as_ref()));
        Self {
            sender,
            state,
            worker: Some(worker),
        }
    }

    #[must_use]
    pub fn state(&self) -> DeviceResult {
        let state = *locked(&self.state);
        state.error.map_or(
            DeviceResult::Audio {
                available: true,
                state: state.playback,
                position_ms: state.position_ms,
                duration_ms: state.duration_ms,
                volume: state.volume,
            },
            DeviceResult::Failed,
        )
    }

    #[must_use]
    pub fn load(&self, source: Source) -> DeviceResult {
        {
            let mut state = locked(&self.state);
            state.playback = AudioPlaybackState::Loading;
            state.position_ms = 0;
            state.duration_ms = 0;
            state.error = None;
        }
        self.send(Command::Load(source))
    }

    #[must_use]
    pub fn play(&self) -> DeviceResult {
        let restart = {
            let mut state = locked(&self.state);
            if state.playback == AudioPlaybackState::Idle {
                return DeviceResult::Failed(DeviceError::NotFound);
            }
            let restart = state.playback == AudioPlaybackState::Finished;
            if restart {
                state.position_ms = 0;
            }
            state.playback = AudioPlaybackState::Playing;
            state.error = None;
            restart
        };
        self.send(Command::Play { restart })
    }

    #[must_use]
    pub fn pause(&self) -> DeviceResult {
        {
            let mut state = locked(&self.state);
            if state.playback == AudioPlaybackState::Playing {
                state.playback = AudioPlaybackState::Paused;
            }
            state.error = None;
        }
        self.send(Command::Pause)
    }

    #[must_use]
    pub fn seek(&self, position_ms: u32) -> DeviceResult {
        {
            let mut state = locked(&self.state);
            if state.playback == AudioPlaybackState::Idle {
                return DeviceResult::Failed(DeviceError::NotFound);
            }
            state.position_ms = position_ms.min(state.duration_ms);
            state.error = None;
        }
        self.send(Command::Seek(position_ms))
    }

    #[must_use]
    pub fn stop(&self) -> DeviceResult {
        {
            let mut state = locked(&self.state);
            state.position_ms = 0;
            state.playback = if state.duration_ms == 0 {
                AudioPlaybackState::Idle
            } else {
                AudioPlaybackState::Ready
            };
            state.error = None;
        }
        self.send(Command::Stop)
    }

    #[must_use]
    pub fn set_volume(&self, percent: u8) -> DeviceResult {
        let percent = percent.min(100);
        {
            let mut state = locked(&self.state);
            state.volume = percent;
            state.error = None;
        }
        self.send(Command::Volume(percent))
    }

    fn send(&self, command: Command) -> DeviceResult {
        if self.sender.send(command).is_err() {
            return DeviceResult::Failed(DeviceError::Backend);
        }
        self.state()
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn locked(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct Track {
    encoded: Vec<u8>,
    frames: u64,
}

struct Media {
    tracks: Vec<Track>,
    starts: Vec<u64>,
    total_frames: u64,
    track: usize,
    pcm: Vec<i16>,
    frame: usize,
}

impl Media {
    fn from_bytes(bytes: Vec<u8>) -> Result<Self, DeviceError> {
        let encoded = if bytes.starts_with(b"PK\x03\x04") {
            let archive = Archive::open(&bytes).map_err(|_| DeviceError::InvalidInput)?;
            let names = archive
                .names()
                .filter(|name| name.to_ascii_lowercase().ends_with(".mp3"))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if names.is_empty() {
                return Err(DeviceError::InvalidInput);
            }
            names
                .iter()
                .map(|name| archive.read(name).map_err(|_| DeviceError::InvalidInput))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            vec![bytes]
        };

        let mut tracks = Vec::with_capacity(encoded.len());
        let mut starts = Vec::with_capacity(encoded.len());
        let mut total_frames = 0_u64;
        for bytes in encoded {
            if bytes.is_empty() || bytes.len() > MAX_TRACK_BYTES {
                return Err(DeviceError::InvalidInput);
            }
            starts.push(total_frames);
            let frames = inspect_frames(&bytes)?;
            if frames == 0 || frames > MAX_TRACK_FRAMES_U64 {
                return Err(DeviceError::InvalidInput);
            }
            total_frames = total_frames
                .checked_add(frames)
                .ok_or(DeviceError::InvalidInput)?;
            tracks.push(Track {
                encoded: bytes,
                frames,
            });
        }
        Ok(Self {
            tracks,
            starts,
            total_frames,
            track: 0,
            pcm: Vec::new(),
            frame: 0,
        })
    }

    fn duration_ms(&self) -> u32 {
        frames_to_ms(self.total_frames)
    }

    fn position_frames(&self) -> u64 {
        self.starts
            .get(self.track)
            .copied()
            .unwrap_or(self.total_frames)
            .saturating_add(self.frame as u64)
            .min(self.total_frames)
    }

    fn ensure_decoded(&mut self) -> Result<(), DeviceError> {
        if self.pcm.is_empty() && self.track < self.tracks.len() {
            self.pcm = decode_track(&self.tracks[self.track].encoded)?;
            let actual = u64::try_from(self.pcm.len() / 2).unwrap_or(u64::MAX);
            let expected = self.tracks[self.track].frames;
            if actual.abs_diff(expected) > TARGET_RATE * 2 {
                return Err(DeviceError::Backend);
            }
            self.tracks[self.track].frames = actual;
        }
        Ok(())
    }

    fn seek(&mut self, position_ms: u32) -> Result<(), DeviceError> {
        let wanted = u64::from(position_ms)
            .saturating_mul(TARGET_RATE)
            .checked_div(1_000)
            .unwrap_or(0)
            .min(self.total_frames);
        let track = self
            .starts
            .iter()
            .enumerate()
            .rfind(|(_, start)| **start <= wanted)
            .map_or(0, |(index, _)| index);
        if track != self.track {
            self.track = track;
            self.pcm.clear();
        }
        self.ensure_decoded()?;
        self.frame = usize::try_from(wanted.saturating_sub(self.starts[self.track]))
            .unwrap_or(usize::MAX)
            .min(self.pcm.len() / 2);
        Ok(())
    }

    fn advance_track(&mut self) -> Result<bool, DeviceError> {
        if self.frame < self.pcm.len() / 2 {
            return Ok(true);
        }
        if self.track + 1 >= self.tracks.len() {
            self.frame = self.pcm.len() / 2;
            return Ok(false);
        }
        self.track += 1;
        self.frame = 0;
        self.pcm.clear();
        self.ensure_decoded()?;
        Ok(true)
    }
}

fn run(receiver: &mpsc::Receiver<Command>, state: &Mutex<State>, fetcher: Option<&StreamFetcher>) {
    let mut media: Option<Media> = None;
    let mut sink: Option<A2dpSink> = None;
    let mut next_write = Instant::now();
    loop {
        let playback = locked(state).playback;
        let wait = if matches!(
            playback,
            AudioPlaybackState::Playing | AudioPlaybackState::Paused
        ) {
            next_write.saturating_duration_since(Instant::now())
        } else {
            Duration::from_secs(86_400)
        };
        match receiver.recv_timeout(wait) {
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                stop_sink(&mut sink);
                break;
            }
            Ok(command) => handle_command(
                command,
                state,
                fetcher,
                &mut media,
                &mut sink,
                &mut next_write,
            ),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let current = locked(state).playback;
                let result = match current {
                    AudioPlaybackState::Playing => play_chunk(state, &mut media, &mut sink),
                    AudioPlaybackState::Paused => keepalive(&mut sink),
                    _ => Ok(()),
                };
                if let Err(error) = result {
                    fail(state, error);
                    stop_sink(&mut sink);
                }
                next_write = next_deadline(next_write, Instant::now());
            }
        }
    }
}

fn handle_command(
    command: Command,
    state: &Mutex<State>,
    fetcher: Option<&StreamFetcher>,
    media: &mut Option<Media>,
    sink: &mut Option<A2dpSink>,
    next_write: &mut Instant,
) {
    match command {
        Command::Load(source) => {
            stop_sink(sink);
            match load_source(source, fetcher).and_then(Media::from_bytes) {
                Ok(loaded) => {
                    let duration_ms = loaded.duration_ms();
                    *media = Some(loaded);
                    let mut observed = locked(state);
                    observed.playback = AudioPlaybackState::Ready;
                    observed.position_ms = 0;
                    observed.duration_ms = duration_ms;
                    observed.error = None;
                }
                Err(error) => fail(state, error),
            }
        }
        Command::Play { restart } => {
            let Some(loaded) = media.as_mut() else {
                fail(state, DeviceError::NotFound);
                return;
            };
            if restart {
                if let Err(error) = loaded.seek(0) {
                    fail(state, error);
                    return;
                }
            }
            if sink.is_none() {
                match A2dpSink::open_and_start() {
                    Ok(mut opened) => {
                        if let Err(error) = opened.write_silence(LEAD_IN_FRAMES) {
                            fail(state, error);
                            return;
                        }
                        *sink = Some(opened);
                        // The silence above is the cushion. Writing the first
                        // real chunk right behind it keeps the cushion full;
                        // waiting it out would start playback with an empty
                        // buffer and a click on the first late chunk.
                        *next_write = Instant::now();
                    }
                    Err(error) => {
                        fail(state, error);
                        return;
                    }
                }
            }
            let mut observed = locked(state);
            observed.playback = AudioPlaybackState::Playing;
            observed.error = None;
        }
        Command::Pause => {
            let mut observed = locked(state);
            if observed.playback == AudioPlaybackState::Playing {
                observed.playback = AudioPlaybackState::Paused;
            }
        }
        Command::Seek(position_ms) => {
            let Some(loaded) = media.as_mut() else {
                fail(state, DeviceError::NotFound);
                return;
            };
            match loaded.seek(position_ms) {
                Ok(()) => {
                    let mut observed = locked(state);
                    observed.position_ms = frames_to_ms(loaded.position_frames());
                    observed.playback = if observed.playback == AudioPlaybackState::Finished {
                        AudioPlaybackState::Paused
                    } else {
                        observed.playback
                    };
                    observed.error = None;
                }
                Err(error) => fail(state, error),
            }
        }
        Command::Stop => {
            stop_sink(sink);
            if let Some(loaded) = media.as_mut() {
                let _ = loaded.seek(0);
            }
            let mut observed = locked(state);
            observed.position_ms = 0;
            observed.playback = if media.is_some() {
                AudioPlaybackState::Ready
            } else {
                AudioPlaybackState::Idle
            };
            observed.error = None;
        }
        Command::Volume(percent) => {
            let mut observed = locked(state);
            observed.volume = percent;
            observed.error = None;
        }
        Command::Shutdown => {}
    }
}

fn load_source(source: Source, fetcher: Option<&StreamFetcher>) -> Result<Vec<u8>, DeviceError> {
    match source {
        Source::File(path) => {
            let size = fs::metadata(&path)
                .map_err(|_| DeviceError::NotFound)?
                .len();
            if size > MAX_SOURCE_BYTES as u64 {
                return Err(DeviceError::InvalidInput);
            }
            fs::read(path).map_err(|_| DeviceError::NotFound)
        }
        Source::Stream(url) => {
            let fetcher = fetcher.ok_or(DeviceError::Unreachable)?;
            let mut bytes = Vec::new();
            loop {
                let offset = u32::try_from(bytes.len()).map_err(|_| DeviceError::InvalidInput)?;
                let chunk = fetcher(&url, offset, FETCH_CHUNK)?;
                let done = chunk.len() < FETCH_CHUNK as usize;
                if bytes.len().saturating_add(chunk.len()) > MAX_SOURCE_BYTES {
                    return Err(DeviceError::InvalidInput);
                }
                bytes.extend_from_slice(&chunk);
                if done {
                    break;
                }
            }
            if bytes.is_empty() {
                Err(DeviceError::NotFound)
            } else {
                Ok(bytes)
            }
        }
    }
}

fn play_chunk(
    state: &Mutex<State>,
    media: &mut Option<Media>,
    sink: &mut Option<A2dpSink>,
) -> Result<(), DeviceError> {
    let loaded = media.as_mut().ok_or(DeviceError::NotFound)?;
    loaded.ensure_decoded()?;
    if !loaded.advance_track()? {
        let mut observed = locked(state);
        observed.position_ms = observed.duration_ms;
        observed.playback = AudioPlaybackState::Finished;
        stop_sink(sink);
        return Ok(());
    }
    let start = loaded.frame.saturating_mul(2);
    let frames = CHUNK_FRAMES.min(loaded.pcm.len() / 2 - loaded.frame);
    let end = start + frames * 2;
    let volume = locked(state).volume;
    sink.as_mut()
        .ok_or(DeviceError::Unreachable)?
        .write_samples(&loaded.pcm[start..end], volume)?;
    loaded.frame += frames;
    locked(state).position_ms = frames_to_ms(loaded.position_frames());
    Ok(())
}

fn keepalive(sink: &mut Option<A2dpSink>) -> Result<(), DeviceError> {
    sink.as_mut()
        .ok_or(DeviceError::Unreachable)?
        .write_silence(CHUNK_FRAMES)
}

fn fail(state: &Mutex<State>, error: DeviceError) {
    let mut observed = locked(state);
    observed.error = Some(error);
    observed.playback = AudioPlaybackState::Idle;
}

fn stop_sink(sink: &mut Option<A2dpSink>) {
    if let Some(mut active) = sink.take() {
        let _ = active.stop();
    }
}

fn frames_to_ms(frames: u64) -> u32 {
    u32::try_from(frames.saturating_mul(1_000) / TARGET_RATE).unwrap_or(u32::MAX)
}

/// When the write after the one just made is due.
///
/// The schedule advances by [`WRITE_PERIOD`] from the previous *deadline*,
/// not from the moment the write finished. Scheduling from the finish time
/// silently adds the cost of decoding and writing to every period, so
/// delivery runs a few percent slower than the device plays and the
/// buffer drains to a steady crackle. A schedule that has fallen more
/// than [`RESYNC_LIMIT`] behind has hit a real stall and restarts at
/// `now`; anything nearer is jitter, and the shortened waits that follow
/// let the writes catch back up to the clock.
fn next_deadline(previous: Instant, now: Instant) -> Instant {
    if now.saturating_duration_since(previous) > RESYNC_LIMIT {
        now
    } else {
        previous + WRITE_PERIOD
    }
}

fn inspect_frames(encoded: &[u8]) -> Result<u64, DeviceError> {
    let mut decoder = Decoder::new(std::io::Cursor::new(encoded));
    let mut source_frames = 0_u64;
    let mut rate = None;
    loop {
        match decoder.next_frame() {
            Ok(frame) => {
                let frame_rate = positive_rate(frame.sample_rate)?;
                if rate.is_some_and(|known| known != frame_rate) || frame.channels == 0 {
                    return Err(DeviceError::InvalidInput);
                }
                rate = Some(frame_rate);
                source_frames = source_frames.saturating_add(
                    u64::try_from(frame.data.len() / frame.channels).unwrap_or(u64::MAX),
                );
            }
            Err(Mp3Error::Eof) => break,
            Err(Mp3Error::Io(_)) => return Err(DeviceError::Backend),
            Err(Mp3Error::InsufficientData | Mp3Error::SkippedData) => {}
        }
    }
    Ok(source_frames.saturating_mul(TARGET_RATE) / rate.ok_or(DeviceError::InvalidInput)?)
}

fn decode_track(encoded: &[u8]) -> Result<Vec<i16>, DeviceError> {
    let mut decoder = Decoder::new(std::io::Cursor::new(encoded));
    let mut native = Vec::new();
    let mut rate = None;
    loop {
        match decoder.next_frame() {
            Ok(frame) => {
                let frame_rate = positive_rate(frame.sample_rate)?;
                if rate.is_some_and(|known| known != frame_rate) || frame.channels == 0 {
                    return Err(DeviceError::InvalidInput);
                }
                rate = Some(frame_rate);
                for samples in frame.data.chunks(frame.channels) {
                    let left = samples[0];
                    let right = samples.get(1).copied().unwrap_or(left);
                    native.extend_from_slice(&[left, right]);
                    if native.len() / 2 > MAX_TRACK_FRAMES {
                        return Err(DeviceError::InvalidInput);
                    }
                }
            }
            Err(Mp3Error::Eof) => break,
            Err(Mp3Error::Io(_)) => return Err(DeviceError::Backend),
            Err(Mp3Error::InsufficientData | Mp3Error::SkippedData) => {}
        }
    }
    let rate = rate.ok_or(DeviceError::InvalidInput)?;
    if native.is_empty() {
        return Err(DeviceError::InvalidInput);
    }
    if rate == TARGET_RATE {
        return Ok(native);
    }
    resample_stereo(&native, rate)
}

fn positive_rate(rate: i32) -> Result<u64, DeviceError> {
    u64::try_from(rate)
        .ok()
        .filter(|rate| *rate > 0)
        .ok_or(DeviceError::InvalidInput)
}

fn resample_stereo(native: &[i16], rate: u64) -> Result<Vec<i16>, DeviceError> {
    let input_frames = native.len() / 2;
    let output_frames = u64::try_from(input_frames)
        .unwrap_or(u64::MAX)
        .saturating_mul(TARGET_RATE)
        .checked_div(rate)
        .ok_or(DeviceError::InvalidInput)?;
    let output_frames = usize::try_from(output_frames).map_err(|_| DeviceError::InvalidInput)?;
    if output_frames > MAX_TRACK_FRAMES {
        return Err(DeviceError::InvalidInput);
    }
    let mut output = Vec::with_capacity(output_frames.saturating_mul(2));
    for frame in 0..output_frames {
        let numerator = u64::try_from(frame)
            .unwrap_or(u64::MAX)
            .saturating_mul(rate);
        let first = usize::try_from(numerator / TARGET_RATE)
            .unwrap_or(usize::MAX)
            .min(input_frames - 1);
        let second = (first + 1).min(input_frames - 1);
        let fraction = i64::try_from(numerator % TARGET_RATE).unwrap_or(0);
        for channel in 0..2 {
            let from = i64::from(native[first * 2 + channel]);
            let to = i64::from(native[second * 2 + channel]);
            let sample = from + (to - from) * fraction / TARGET_RATE_I64;
            output.push(
                i16::try_from(sample.clamp(i64::from(i16::MIN), i64::from(i16::MAX)))
                    .expect("the sample was clamped to i16"),
            );
        }
    }
    Ok(output)
}

struct A2dpSink {
    control: UnixStream,
    data: Option<UnixStream>,
    started: bool,
    bytes: Vec<u8>,
}

impl A2dpSink {
    fn open_and_start() -> Result<Self, DeviceError> {
        let control = UnixStream::connect(CONTROL_SOCKET).map_err(io_error)?;
        control
            .set_read_timeout(Some(IO_TIMEOUT))
            .map_err(io_error)?;
        control
            .set_write_timeout(Some(IO_TIMEOUT))
            .map_err(io_error)?;
        let mut sink = Self {
            control,
            data: None,
            started: false,
            bytes: Vec::new(),
        };
        sink.command(COMMAND_CHECK_READY, true)?;
        sink.command(COMMAND_START, true)?;
        // START has changed btservice state even before the data listener is
        // reachable. Mark it now so a failed data connect still sends STOP
        // from Drop instead of leaving the firmware stream wedged.
        sink.started = true;
        let deadline = Instant::now() + DATA_CONNECT_TIMEOUT;
        loop {
            match UnixStream::connect(DATA_SOCKET) {
                Ok(data) => {
                    data.set_write_timeout(Some(IO_TIMEOUT)).map_err(io_error)?;
                    sink.data = Some(data);
                    return Ok(sink);
                }
                Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                Err(error) => return Err(io_error(error)),
            }
        }
    }

    fn command(&mut self, command: u8, check_ack: bool) -> Result<(), DeviceError> {
        self.control.write_all(&[command]).map_err(io_error)?;
        let mut ack = [0_u8; 1];
        self.control.read_exact(&mut ack).map_err(io_error)?;
        if check_ack && ack[0] != ACK_SUCCESS {
            return Err(DeviceError::Unreachable);
        }
        Ok(())
    }

    fn write_samples(&mut self, samples: &[i16], volume: u8) -> Result<(), DeviceError> {
        self.bytes.clear();
        self.bytes.reserve(samples.len().saturating_mul(2));
        for sample in samples {
            let scaled = i32::from(*sample) * i32::from(volume) / 100;
            self.bytes.extend_from_slice(
                &i16::try_from(scaled)
                    .expect("a percentage cannot enlarge an i16 sample")
                    .to_le_bytes(),
            );
        }
        self.data
            .as_mut()
            .ok_or(DeviceError::Unreachable)?
            .write_all(&self.bytes)
            .map_err(io_error)
    }

    fn write_silence(&mut self, frames: usize) -> Result<(), DeviceError> {
        self.bytes.clear();
        self.bytes.resize(frames.saturating_mul(4), 0);
        self.data
            .as_mut()
            .ok_or(DeviceError::Unreachable)?
            .write_all(&self.bytes)
            .map_err(io_error)
    }

    fn stop(&mut self) -> Result<(), DeviceError> {
        if self.started {
            self.command(COMMAND_STOP, false)?;
            self.started = false;
        }
        Ok(())
    }
}

impl Drop for A2dpSink {
    fn drop(&mut self) {
        if self.started {
            let _ = self.command(COMMAND_STOP, false);
            self.started = false;
        }
    }
}

fn io_error(error: std::io::Error) -> DeviceError {
    let kind = error.kind();
    drop(error);
    match kind {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
            DeviceError::Unreachable
        }
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => DeviceError::TimedOut,
        _ => DeviceError::Backend,
    }
}

#[cfg(test)]
mod tests {
    use super::{frames_to_ms, next_deadline, resample_stereo, Audio, BACKEND_MARKERS};
    use super::{RESYNC_LIMIT, WRITE_PERIOD};
    use std::time::Instant;

    #[test]
    fn clara_bw_stable_a2dp_hal_marker_is_known() {
        assert!(BACKEND_MARKERS.contains(&"/usr/lib/libaudio.a2dp.default.so"));
    }

    #[test]
    fn the_write_cadence_does_not_absorb_the_cost_of_the_writes() {
        let start = Instant::now();
        // The chunk took 8ms to produce and deliver; the next deadline still
        // sits one whole period after the previous one.
        let after_work = start + std::time::Duration::from_millis(8);
        assert_eq!(next_deadline(start, after_work), start + WRITE_PERIOD);
    }

    #[test]
    fn a_stalled_schedule_restarts_instead_of_bursting() {
        let start = Instant::now();
        let much_later = start + RESYNC_LIMIT + WRITE_PERIOD;
        assert_eq!(next_deadline(start, much_later), much_later);
    }

    #[test]
    fn resampling_preserves_channels_and_duration() {
        let input = [100_i16, -100].repeat(24_000);
        let output = resample_stereo(&input, 24_000).expect("resample");
        assert_eq!(output.len(), 48_000 * 2);
        assert_eq!(&output[..2], &[100, -100]);
    }

    #[test]
    fn frames_are_reported_in_milliseconds_without_overflow() {
        assert_eq!(frames_to_ms(48_000), 1_000);
        assert_eq!(frames_to_ms(u64::MAX), u32::MAX);
    }

    #[test]
    fn a_worker_can_start_and_shut_down_without_hardware() {
        let audio = Audio::simulated();
        drop(audio);
    }
}
