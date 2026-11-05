//! Runtime-owned audiobook playback through Kobo's A2DP HAL.
//!
//! Bluetooth connection ownership stays with the firmware's `btservice`.
//! Once an audio device is connected it exposes the standard AOSP
//! `audio_a2dp_hw` control and data sockets. The player sends CHECK_READY,
//! START and STOP on the control socket and paced S16LE PCM on the data
//! socket, at the rate the stack asks for. No Nickel process or audio
//! command-line utility is required.
//!
//! A book is decoded as it plays, a chunk ahead of the sink, by
//! [`kobo_audio`]. What this module owns is the clock, the sockets, the
//! volume, and the facts about the sink that nothing else can learn.

use kobo_audio::resample::Resampler;
use kobo_audio::stretch::Stretcher;
use kobo_audio::{Book, Description, Fault};
use kobo_protocol::{AudioPlaybackState, DeviceError, DeviceResult};
use std::collections::VecDeque;
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
/// Asks the stack what it reads from the data socket: a rate and a channel
/// count, after the acknowledgement. The firmware's own HAL asks this of
/// the stack when it opens a stream, and it is the only way to know rather
/// than guess.
const COMMAND_GET_AUDIO_CONFIG: u8 = 0x05;
const ACK_SUCCESS: u8 = 0x00;
/// What the firmware's Bluetooth encoder consumes when it will not say.
/// Found empirically on the Clara BW: a 44.1 kHz sine over the data socket
/// stutters and plays sharp, the same tone generated at 48 kHz plays clean
/// and true, so the encoder reads the stream as 48 kHz unless it answers
/// [`COMMAND_GET_AUDIO_CONFIG`] with something else.
const DEFAULT_RATE: u32 = 48_000;
/// How much audio one chunk carries, and therefore the write cadence.
const WRITE_PERIOD: Duration = Duration::from_millis(50);
const CHUNK_MS: u32 = 50;
/// Silence written ahead of the first chunk, so a late chunk is a late
/// chunk and not a click.
const LEAD_IN_MS: u32 = 500;
/// A schedule this far behind is a stall, not jitter, and is restarted
/// rather than caught up with a burst of writes.
const RESYNC_LIMIT: Duration = Duration::from_millis(500);
/// How long a paused book keeps the stream open with silence.
///
/// Headphones switch themselves off after a few minutes without a stream
/// and the stack closes its data socket the moment the stream stops, so a
/// pause keeps the stream alive with silence, which is what makes
/// resuming instant. It cannot keep it alive forever: silence costs the
/// radio and the headphones' battery as much as speech does. After this
/// long the stream is released, and the next Play opens it again.
const PAUSE_KEEPALIVE: Duration = Duration::from_secs(300);
/// How long a Play keeps asking the stack for its sink before concluding
/// that the headphones are not there.
///
/// A Bluetooth connect answers as soon as the link is up. The audio channel
/// over that link is negotiated afterwards, and on the `MediaTek` stack that
/// has been seen to take anything up to five seconds. A Play that lands in
/// the gap, which is where every Play lands that waited for the headphones
/// to connect, finds the stack not ready. Asking again every half second
/// for this long makes that a short wait rather than a complaint.
const SINK_PATIENCE: Duration = Duration::from_secs(5);
const SINK_RETRY: Duration = Duration::from_millis(500);
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const DATA_CONNECT_TIMEOUT: Duration = Duration::from_millis(750);
/// The largest stream cached before playing. A stream is fetched whole
/// because the runtime's fetch is a range of bytes, not a socket.
const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;
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

/// Where the stack's two sockets are.
#[derive(Clone, Debug)]
struct Sockets {
    control: PathBuf,
    data: PathBuf,
}

impl Sockets {
    fn firmware() -> Self {
        Self {
            control: PathBuf::from(CONTROL_SOCKET),
            data: PathBuf::from(DATA_SOCKET),
        }
    }
}

/// Bounded range fetch used by stream sources. The runtime injects its own TLS
/// transport, keeping credentials and sockets out of SDK applications.
pub type StreamFetcher = Arc<dyn Fn(&str, u32, u32) -> Result<Vec<u8>, DeviceError> + Send + Sync>;

/// Where a book comes from.
#[derive(Clone, Debug)]
pub enum Source {
    /// One file: a single book, or an `.mp3z` of parts.
    File(PathBuf),
    /// A folder's files, in playing order, as one book called `title`.
    Parts {
        title: String,
        paths: Vec<PathBuf>,
    },
    Stream(String),
}

/// One thing playback proved about the Bluetooth sink.
///
/// Numbered, because a repeat is still news. Playback can fail to open the
/// sink twice in a row with a Bluetooth reply saying "connected" in between,
/// and a reader of a bare `false` cannot tell the second failure from the
/// first, so it would leave the mark showing headphones that are gone. A new
/// `observation` is a new fact whatever it says.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SinkEvidence {
    /// Counts up with every attempt that proved something.
    pub observation: u64,
    /// Whether the sink accepted audio.
    pub connected: bool,
}

#[derive(Clone, Debug)]
struct State {
    playback: AudioPlaybackState,
    position_ms: u32,
    duration_ms: u32,
    volume: u8,
    speed_percent: u16,
    error: Option<DeviceError>,
    /// What the last attempt to use the A2DP sink proved, if anything.
    sink: Option<SinkEvidence>,
    /// What the loaded book says about itself.
    description: Option<Arc<Description>>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            playback: AudioPlaybackState::Idle,
            position_ms: 0,
            duration_ms: 0,
            volume: 70,
            speed_percent: 100,
            error: None,
            sink: None,
            description: None,
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
    Speed(u16),
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
        Some(Self::spawn(fetcher, Sockets::firmware(), SINK_PATIENCE))
    }

    /// A player with no stack behind it, and little patience for one: a
    /// host has no sockets, and a test should not wait five seconds to
    /// learn that.
    #[cfg(test)]
    fn simulated() -> Self {
        Self::spawn(None, Sockets::firmware(), Duration::from_millis(600))
    }

    #[cfg(test)]
    fn simulated_with(sockets: Sockets, patience: Duration) -> Self {
        Self::spawn(None, sockets, patience)
    }

    fn spawn(fetcher: Option<StreamFetcher>, sockets: Sockets, patience: Duration) -> Self {
        let (sender, receiver) = mpsc::channel();
        let state = Arc::new(Mutex::new(State::default()));
        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            run(
                &receiver,
                &worker_state,
                fetcher.as_ref(),
                &sockets,
                patience,
            );
        });
        Self {
            sender,
            state,
            worker: Some(worker),
        }
    }

    /// Whether the firmware's Bluetooth audio sink was last seen accepting
    /// audio.
    ///
    /// The A2DP sockets exist only while headphones are connected, so opening
    /// them is proof of a connection and losing them mid-stream is proof of
    /// its end. `None` until playback has tried, because a player that has
    /// never opened the sink knows nothing about the headphones either way.
    /// Stopping playback deliberately leaves the last answer in place: the
    /// headphones are still there when a book is paused.
    #[must_use]
    pub fn sink_evidence(&self) -> Option<SinkEvidence> {
        locked(&self.state).sink
    }

    /// Whether a book is playing right now, which is the one thing about
    /// playback the power policy needs to know.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        locked(&self.state).playback == AudioPlaybackState::Playing
    }

    /// What the loaded book says about itself, once it is loaded.
    #[must_use]
    pub fn detail(&self) -> Option<Arc<Description>> {
        locked(&self.state).description.clone()
    }

    /// The speed the book plays at, as a percentage.
    #[must_use]
    pub fn speed_percent(&self) -> u16 {
        locked(&self.state).speed_percent
    }

    #[must_use]
    pub fn state(&self) -> DeviceResult {
        let state = locked(&self.state);
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
            state.description = None;
        }
        self.send(Command::Load(source))
    }

    #[must_use]
    pub fn play(&self) -> DeviceResult {
        let restart = {
            let mut state = locked(&self.state);
            if matches!(
                state.playback,
                AudioPlaybackState::Idle | AudioPlaybackState::Loading
            ) {
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
            if matches!(
                state.playback,
                AudioPlaybackState::Idle | AudioPlaybackState::Loading
            ) {
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

    /// Sets the playing speed as a percentage, within what the stretcher
    /// does well. The state answered says the speed that was kept.
    #[must_use]
    pub fn set_speed(&self, percent: u16) -> DeviceResult {
        let percent = percent.clamp(
            kobo_audio::stretch::MIN_SPEED_PERCENT,
            kobo_audio::stretch::MAX_SPEED_PERCENT,
        );
        {
            let mut state = locked(&self.state);
            state.speed_percent = percent;
            state.error = None;
        }
        self.send(Command::Speed(percent))
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

/// A book and the stages between it and the sink: the resampler to the
/// sink's rate, the stretcher for the chosen speed, and the frames made
/// ready ahead of the next write.
struct Media {
    book: Book,
    sink_rate: u32,
    resampler: Resampler,
    stretcher: Stretcher,
    native: Vec<i16>,
    resampled: Vec<i16>,
    stretched: Vec<i16>,
    /// Interleaved stereo at `sink_rate`, ready to write.
    ready: VecDeque<i16>,
}

impl Media {
    fn new(book: Book, sink_rate: u32, speed_percent: u16) -> Self {
        let mut stretcher = Stretcher::new(sink_rate);
        stretcher.set_speed(speed_percent);
        Self {
            book,
            sink_rate,
            resampler: Resampler::new(sink_rate, sink_rate),
            stretcher,
            native: Vec::new(),
            resampled: Vec::new(),
            stretched: Vec::new(),
            ready: VecDeque::new(),
        }
    }

    /// Follows the sink to a different rate, throwing away what was made
    /// for the old one.
    fn retune(&mut self, sink_rate: u32) {
        if sink_rate == self.sink_rate {
            return;
        }
        let speed = self.stretcher.speed_percent();
        self.sink_rate = sink_rate;
        self.resampler = Resampler::new(self.resampler.from_rate(), sink_rate);
        self.stretcher = Stretcher::new(sink_rate);
        self.stretcher.set_speed(speed);
        self.ready.clear();
    }

    fn seek(&mut self, position_ms: u32) -> Result<(), DeviceError> {
        self.book.seek(position_ms).map_err(fault_error)?;
        self.resampler.reset();
        self.stretcher.reset();
        self.ready.clear();
        Ok(())
    }

    /// Makes at least `frames` frames ready, or everything the book has
    /// left. Returns whether anything at all is ready.
    fn fill(&mut self, frames: usize) -> Result<bool, DeviceError> {
        while self.ready.len() / 2 < frames {
            self.native.clear();
            let sound = self
                .book
                .read(frames, &mut self.native)
                .map_err(fault_error)?;
            if sound.frames == 0 {
                // The stretcher keeps up to one frame it cannot match yet;
                // a book's last thirty milliseconds are its silence anyway.
                break;
            }
            if sound.rate != self.resampler.from_rate() {
                self.resampler = Resampler::new(sound.rate, self.sink_rate);
            }
            self.resampled.clear();
            self.resampler.push(&self.native, &mut self.resampled);
            self.stretched.clear();
            self.stretcher.push(&self.resampled, &mut self.stretched);
            self.ready.extend(self.stretched.iter().copied());
        }
        Ok(!self.ready.is_empty())
    }

    /// Takes up to `frames` ready frames as a contiguous slice.
    fn take(&mut self, frames: usize, out: &mut Vec<i16>) {
        out.clear();
        let take = frames.min(self.ready.len() / 2) * 2;
        out.extend(self.ready.drain(..take));
    }
}

fn fault_error(fault: Fault) -> DeviceError {
    match fault {
        Fault::Unreadable => DeviceError::NotFound,
        Fault::Unsupported => DeviceError::InvalidInput,
        Fault::Damaged => DeviceError::Backend,
    }
}

/// A Play whose sink has not opened yet.
#[derive(Clone, Copy)]
struct SinkWait {
    /// When Play was pressed, from which the patience is counted.
    since: Instant,
    /// When the stack is next asked.
    next_try: Instant,
}

/// Everything the worker thread holds between commands.
struct Worker<'a> {
    state: &'a Mutex<State>,
    fetcher: Option<&'a StreamFetcher>,
    sockets: &'a Sockets,
    /// How long a Play waits for the sink.
    patience: Duration,
    media: Option<Media>,
    sink: Option<A2dpSink>,
    /// A Play still waiting for the stack to be ready.
    waiting: Option<SinkWait>,
    next_write: Instant,
    /// When the book was paused, while the stream is kept alive.
    paused_at: Option<Instant>,
    chunk: Vec<i16>,
}

fn run(
    receiver: &mpsc::Receiver<Command>,
    state: &Mutex<State>,
    fetcher: Option<&StreamFetcher>,
    sockets: &Sockets,
    patience: Duration,
) {
    let mut worker = Worker {
        state,
        fetcher,
        sockets,
        patience,
        media: None,
        sink: None,
        waiting: None,
        next_write: Instant::now(),
        paused_at: None,
        chunk: Vec::new(),
    };
    loop {
        let playback = locked(state).playback;
        let ticking = match playback {
            AudioPlaybackState::Playing => true,
            AudioPlaybackState::Paused => worker.sink.is_some(),
            _ => false,
        };
        let wait = if ticking {
            worker.next_write.saturating_duration_since(Instant::now())
        } else {
            Duration::from_secs(86_400)
        };
        match receiver.recv_timeout(wait) {
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                stop_sink(&mut worker.sink);
                break;
            }
            Ok(command) => worker.handle(command),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                worker.tick();
                worker.next_write = next_deadline(worker.next_write, Instant::now());
            }
        }
    }
}

impl Worker<'_> {
    fn handle(&mut self, command: Command) {
        match command {
            Command::Load(source) => self.load(source),
            Command::Play { restart } => self.play(restart),
            Command::Pause => {
                let mut observed = locked(self.state);
                if observed.playback == AudioPlaybackState::Playing {
                    observed.playback = AudioPlaybackState::Paused;
                }
                drop(observed);
                self.waiting = None;
                self.paused_at = Some(Instant::now());
            }
            Command::Seek(position_ms) => self.seek(position_ms),
            Command::Stop => {
                stop_sink(&mut self.sink);
                self.waiting = None;
                self.paused_at = None;
                if let Some(media) = self.media.as_mut() {
                    let _ = media.seek(0);
                }
                let mut observed = locked(self.state);
                observed.position_ms = 0;
                observed.playback = if self.media.is_some() {
                    AudioPlaybackState::Ready
                } else {
                    AudioPlaybackState::Idle
                };
                observed.error = None;
            }
            Command::Volume(percent) => {
                let mut observed = locked(self.state);
                observed.volume = percent;
                observed.error = None;
            }
            Command::Speed(percent) => {
                if let Some(media) = self.media.as_mut() {
                    media.stretcher.set_speed(percent);
                }
                let mut observed = locked(self.state);
                observed.speed_percent = percent;
                observed.error = None;
            }
            Command::Shutdown => {}
        }
    }

    fn load(&mut self, source: Source) {
        stop_sink(&mut self.sink);
        self.waiting = None;
        self.paused_at = None;
        self.media = None;
        let speed = locked(self.state).speed_percent;
        match open_source(source, self.fetcher) {
            Ok(book) => {
                let description = Arc::new(book.description().clone());
                let duration_ms = book.duration_ms();
                self.media = Some(Media::new(book, DEFAULT_RATE, speed));
                let mut observed = locked(self.state);
                observed.playback = AudioPlaybackState::Ready;
                observed.position_ms = 0;
                observed.duration_ms = duration_ms;
                observed.description = Some(description);
                observed.error = None;
            }
            Err(error) => fail(self.state, error),
        }
    }

    fn play(&mut self, restart: bool) {
        let Some(media) = self.media.as_mut() else {
            fail(self.state, DeviceError::NotFound);
            return;
        };
        if restart {
            if let Err(error) = media.seek(0) {
                pause_with(self.state, error);
                return;
            }
        }
        self.paused_at = None;
        {
            let mut observed = locked(self.state);
            observed.playback = AudioPlaybackState::Playing;
            observed.error = None;
        }
        if self.sink.is_none() {
            let now = Instant::now();
            self.waiting = Some(SinkWait {
                since: now,
                next_try: now,
            });
            self.try_sink();
        }
    }

    /// One attempt to open the sink for the Play that is waiting on it.
    ///
    /// A sink that opens ends the wait and starts the stream. One that
    /// refuses is asked again after [`SINK_RETRY`] until the patience runs
    /// out, and only then is the refusal taken as the headphones being gone
    /// and the book paused where it is.
    fn try_sink(&mut self) {
        let Some(wait) = self.waiting else {
            return;
        };
        let now = Instant::now();
        if now < wait.next_try {
            return;
        }
        let attempt = A2dpSink::open_and_start(self.sockets).and_then(|mut opened| {
            opened
                .write_silence(opened.frames_in_ms(LEAD_IN_MS))
                .map(|()| opened)
        });
        match attempt {
            Ok(opened) => {
                if let Some(media) = self.media.as_mut() {
                    media.retune(opened.rate);
                }
                self.sink = Some(opened);
                self.waiting = None;
                record_sink(self.state, true);
                // The silence written above is the cushion. Writing the
                // first real chunk right behind it keeps the cushion full;
                // waiting it out would start playback with an empty buffer
                // and a click on the first late chunk.
                self.next_write = Instant::now();
            }
            Err(_) if now.saturating_duration_since(wait.since) < self.patience => {
                self.waiting = Some(SinkWait {
                    since: wait.since,
                    next_try: now + SINK_RETRY,
                });
            }
            Err(error) => {
                self.waiting = None;
                record_sink(self.state, false);
                pause_with(self.state, error);
            }
        }
    }

    fn seek(&mut self, position_ms: u32) {
        let Some(media) = self.media.as_mut() else {
            fail(self.state, DeviceError::NotFound);
            return;
        };
        match media.seek(position_ms) {
            Ok(()) => {
                let mut observed = locked(self.state);
                observed.position_ms = media.book.position_ms();
                if observed.playback == AudioPlaybackState::Finished {
                    observed.playback = AudioPlaybackState::Paused;
                }
                observed.error = None;
            }
            Err(error) => pause_with(self.state, error),
        }
    }

    /// One write period has passed: the next chunk of the book, or of
    /// silence while paused, goes to the sink.
    fn tick(&mut self) {
        if self.waiting.is_some() {
            self.try_sink();
            return;
        }
        let playback = locked(self.state).playback;
        let result = match playback {
            AudioPlaybackState::Playing => self.play_chunk(),
            AudioPlaybackState::Paused => self.keepalive(),
            _ => Ok(()),
        };
        if let Err(error) = result {
            // A sink that stopped taking sound is headphones that went
            // away, and a book that cannot be decoded is a damaged book.
            // Neither loses the place: the book stays loaded and paused,
            // and Play tries again from here.
            pause_with(self.state, error);
            stop_sink(&mut self.sink);
            self.paused_at = None;
        }
    }

    fn play_chunk(&mut self) -> Result<(), DeviceError> {
        let sink = self.sink.as_mut().ok_or(DeviceError::Unreachable)?;
        let media = self.media.as_mut().ok_or(DeviceError::NotFound)?;
        let frames = sink.frames_in_ms(CHUNK_MS);
        if !media.fill(frames)? {
            let mut observed = locked(self.state);
            observed.position_ms = observed.duration_ms;
            observed.playback = AudioPlaybackState::Finished;
            drop(observed);
            stop_sink(&mut self.sink);
            return Ok(());
        }
        media.take(frames, &mut self.chunk);
        let volume = locked(self.state).volume;
        let written = sink.write_samples(&self.chunk, volume);
        sink_write(self.state, written)?;
        locked(self.state).position_ms = media.book.position_ms();
        Ok(())
    }

    fn keepalive(&mut self) -> Result<(), DeviceError> {
        if self
            .paused_at
            .is_some_and(|since| since.elapsed() >= PAUSE_KEEPALIVE)
        {
            stop_sink(&mut self.sink);
            self.paused_at = None;
            return Ok(());
        }
        let sink = self.sink.as_mut().ok_or(DeviceError::Unreachable)?;
        let frames = sink.frames_in_ms(CHUNK_MS);
        let written = sink.write_silence(frames);
        sink_write(self.state, written)
    }
}

fn open_source(source: Source, fetcher: Option<&StreamFetcher>) -> Result<Book, DeviceError> {
    match source {
        Source::File(path) => Book::open(&path).map_err(fault_error),
        Source::Parts { title, paths } => Book::open_parts(&title, &paths).map_err(fault_error),
        Source::Stream(url) => {
            let bytes = fetch_stream(&url, fetcher)?;
            let name = url
                .rsplit('/')
                .next()
                .filter(|name| kobo_audio::Container::from_name(name).is_some())
                .unwrap_or("stream.mp3");
            Book::open_bytes(bytes, name).map_err(fault_error)
        }
    }
}

fn fetch_stream(url: &str, fetcher: Option<&StreamFetcher>) -> Result<Vec<u8>, DeviceError> {
    let fetcher = fetcher.ok_or(DeviceError::Unreachable)?;
    let mut bytes = Vec::new();
    loop {
        let offset = u32::try_from(bytes.len()).map_err(|_| DeviceError::InvalidInput)?;
        let chunk = fetcher(url, offset, FETCH_CHUNK)?;
        let done = chunk.len() < FETCH_CHUNK as usize;
        if bytes.len().saturating_add(chunk.len()) > MAX_STREAM_BYTES {
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

/// Records one new observation of the sink.
fn record_sink(state: &Mutex<State>, connected: bool) {
    let mut observed = locked(state);
    let observation = observed
        .sink
        .map_or(1, |previous| previous.observation.wrapping_add(1));
    observed.sink = Some(SinkEvidence {
        observation,
        connected,
    });
}

/// Passes on the result of writing to an open sink, recording a failure as
/// the headphones going away.
///
/// Only a failed write is that evidence. Playback can also fail before it
/// writes anything, decoding a damaged later part for instance, and taking
/// the headphones off the band then would describe a fault in the book as a
/// change in the hardware.
fn sink_write(state: &Mutex<State>, written: Result<(), DeviceError>) -> Result<(), DeviceError> {
    if written.is_err() {
        record_sink(state, false);
    }
    written
}

/// Nothing is loaded any more: a load that failed.
fn fail(state: &Mutex<State>, error: DeviceError) {
    let mut observed = locked(state);
    observed.error = Some(error);
    observed.playback = AudioPlaybackState::Idle;
    observed.description = None;
}

/// The book is still loaded and its place is kept; only the sound stopped.
fn pause_with(state: &Mutex<State>, error: DeviceError) {
    let mut observed = locked(state);
    observed.error = Some(error);
    if matches!(
        observed.playback,
        AudioPlaybackState::Playing | AudioPlaybackState::Finished
    ) {
        observed.playback = AudioPlaybackState::Paused;
    }
}

fn stop_sink(sink: &mut Option<A2dpSink>) {
    if let Some(mut active) = sink.take() {
        let _ = active.stop();
    }
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

struct A2dpSink {
    control: UnixStream,
    data: Option<UnixStream>,
    started: bool,
    bytes: Vec<u8>,
    /// What the stack reads from the data socket.
    rate: u32,
    channels: u8,
}

impl A2dpSink {
    fn open_and_start(sockets: &Sockets) -> Result<Self, DeviceError> {
        let mut sink = Self {
            control: connect_control(&sockets.control)?,
            data: None,
            started: false,
            bytes: Vec::new(),
            rate: DEFAULT_RATE,
            channels: 2,
        };
        sink.command(COMMAND_CHECK_READY, true)?;
        sink.read_config(&sockets.control);
        sink.command(COMMAND_START, true)?;
        // START has changed btservice state even before the data listener is
        // reachable. Mark it now so a failed data connect still sends STOP
        // from Drop instead of leaving the firmware stream wedged.
        sink.started = true;
        let deadline = Instant::now() + DATA_CONNECT_TIMEOUT;
        loop {
            match UnixStream::connect(&sockets.data) {
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

    /// Asks the stack for its rate and channel count, and keeps the
    /// defaults when it will not say.
    ///
    /// A stack that does not know the command acknowledges it with a
    /// failure and nothing else, which is read and left. A stack that
    /// answers nothing at all, or answers with a rate no encoder uses, has
    /// left the control socket in a state nothing here can trust, so the
    /// socket is opened again before START.
    fn read_config(&mut self, control_socket: &Path) {
        match self.query_config() {
            Ok(Some((rate, channels))) => {
                self.rate = rate;
                self.channels = channels;
            }
            Ok(None) => {}
            Err(_) => {
                if let Ok(control) = connect_control(control_socket) {
                    self.control = control;
                }
            }
        }
    }

    fn query_config(&mut self) -> Result<Option<(u32, u8)>, DeviceError> {
        self.control
            .write_all(&[COMMAND_GET_AUDIO_CONFIG])
            .map_err(io_error)?;
        let mut ack = [0_u8; 1];
        self.control.read_exact(&mut ack).map_err(io_error)?;
        if ack[0] != ACK_SUCCESS {
            return Ok(None);
        }
        let mut answer = [0_u8; 5];
        self.control.read_exact(&mut answer).map_err(io_error)?;
        let rate = u32::from_ne_bytes([answer[0], answer[1], answer[2], answer[3]]);
        let channels = answer[4];
        if !(8_000..=192_000).contains(&rate) || !(1..=2).contains(&channels) {
            return Err(DeviceError::Backend);
        }
        Ok(Some((rate, channels)))
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

    /// How many frames `ms` milliseconds hold at the sink's rate.
    fn frames_in_ms(&self, ms: u32) -> usize {
        usize::try_from(u64::from(self.rate) * u64::from(ms) / 1_000).unwrap_or(0)
    }

    /// Writes interleaved stereo frames, scaled to `volume`, in the channel
    /// layout the stack asked for.
    fn write_samples(&mut self, stereo: &[i16], volume: u8) -> Result<(), DeviceError> {
        self.bytes.clear();
        self.bytes.reserve(stereo.len().saturating_mul(2));
        let scale = |sample: i16| -> [u8; 2] {
            let scaled = i32::from(sample) * i32::from(volume) / 100;
            i16::try_from(scaled)
                .expect("a percentage cannot enlarge an i16 sample")
                .to_le_bytes()
        };
        if self.channels == 1 {
            for frame in stereo.chunks_exact(2) {
                let mixed = (i32::from(frame[0]) + i32::from(frame[1])) / 2;
                self.bytes.extend_from_slice(&scale(
                    i16::try_from(mixed).expect("the mean of two i16 is an i16"),
                ));
            }
        } else {
            for sample in stereo {
                self.bytes.extend_from_slice(&scale(*sample));
            }
        }
        self.data
            .as_mut()
            .ok_or(DeviceError::Unreachable)?
            .write_all(&self.bytes)
            .map_err(io_error)
    }

    fn write_silence(&mut self, frames: usize) -> Result<(), DeviceError> {
        self.bytes.clear();
        self.bytes
            .resize(frames.saturating_mul(2 * usize::from(self.channels)), 0);
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

fn connect_control(socket: &Path) -> Result<UnixStream, DeviceError> {
    let control = UnixStream::connect(socket).map_err(io_error)?;
    control
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(io_error)?;
    control
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(io_error)?;
    Ok(control)
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
    use super::{next_deadline, Audio, Sockets, Source, BACKEND_MARKERS};
    use super::{ACK_SUCCESS, COMMAND_CHECK_READY, COMMAND_GET_AUDIO_CONFIG, COMMAND_START};
    use super::{RESYNC_LIMIT, WRITE_PERIOD};
    use kobo_protocol::{AudioPlaybackState, DeviceError, DeviceResult};
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../kobo-audio/fixtures")
            .join(name)
    }

    /// Waits for the worker to leave `Loading`, which it does on its own
    /// thread.
    fn settled(audio: &Audio) -> DeviceResult {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let state = audio.state();
            let loading = matches!(
                state,
                DeviceResult::Audio {
                    state: AudioPlaybackState::Loading,
                    ..
                }
            );
            if !loading || Instant::now() > deadline {
                return state;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

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
    fn nothing_is_known_about_the_headphones_until_playback_tries() {
        let audio = Audio::simulated();
        assert_eq!(audio.sink_evidence(), None);
    }

    #[test]
    fn a_book_loads_with_its_length_and_what_it_says_about_itself() {
        let audio = Audio::simulated();
        let _ = audio.load(Source::File(fixture("chapters.m4b")));
        let DeviceResult::Audio {
            state,
            duration_ms,
            position_ms,
            ..
        } = settled(&audio)
        else {
            panic!("a load answers with the transport state");
        };
        assert_eq!(state, AudioPlaybackState::Ready);
        assert!((5_900..=6_100).contains(&duration_ms), "{duration_ms}");
        assert_eq!(position_ms, 0);
        let detail = audio.detail().expect("the book describes itself");
        assert_eq!(detail.title.as_deref(), Some("The Quiet Shelf"));
        assert_eq!(detail.chapters.len(), 3);
    }

    #[test]
    fn a_folder_loads_as_one_book_with_a_chapter_a_file() {
        let audio = Audio::simulated();
        let _ = audio.load(Source::Parts {
            title: "Two Tones".to_owned(),
            paths: vec![fixture("pcm.wav"), fixture("lossless.flac")],
        });
        let DeviceResult::Audio { duration_ms, .. } = settled(&audio) else {
            panic!("a load answers with the transport state");
        };
        assert!((3_900..=4_100).contains(&duration_ms), "{duration_ms}");
        let detail = audio.detail().expect("described");
        assert_eq!(detail.title.as_deref(), Some("Two Tones"));
        assert_eq!(detail.chapters.len(), 2);
    }

    #[test]
    fn a_file_that_is_not_a_book_fails_the_load_and_loads_nothing() {
        let audio = Audio::simulated();
        let _ = audio.load(Source::File(fixture("cover.png")));
        assert_eq!(
            settled(&audio),
            DeviceResult::Failed(DeviceError::InvalidInput)
        );
        assert!(audio.detail().is_none());
        assert_eq!(audio.play(), DeviceResult::Failed(DeviceError::NotFound));
    }

    /// Polls `condition` until it holds or `limit` passes.
    fn wait_for(condition: impl Fn() -> bool, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if condition() {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Stands in for the firmware's stack: its two sockets, answering the
    /// control commands the way the stack does, with the number of
    /// `CHECK_READY` commands to refuse before accepting one chosen by the
    /// test. Whatever reaches the data socket is read and counted.
    struct FakeStack {
        sockets: Sockets,
        checks: Arc<AtomicUsize>,
        configs: Arc<AtomicUsize>,
        starts: Arc<AtomicUsize>,
        data_bytes: Arc<AtomicUsize>,
    }

    impl FakeStack {
        fn listen(name: &str, refusals: usize, rate: u32, channels: u8) -> Self {
            let dir = std::env::temp_dir().join(format!("kobo-a2dp-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("a directory for the sockets");
            let sockets = Sockets {
                control: dir.join("ctrl"),
                data: dir.join("data"),
            };
            let control = UnixListener::bind(&sockets.control).expect("a control socket");
            let data = UnixListener::bind(&sockets.data).expect("a data socket");
            let stack = Self {
                sockets,
                checks: Arc::new(AtomicUsize::new(0)),
                configs: Arc::new(AtomicUsize::new(0)),
                starts: Arc::new(AtomicUsize::new(0)),
                data_bytes: Arc::new(AtomicUsize::new(0)),
            };
            let refusals = AtomicUsize::new(refusals);
            let checks = Arc::clone(&stack.checks);
            let configs = Arc::clone(&stack.configs);
            let starts = Arc::clone(&stack.starts);
            std::thread::spawn(move || {
                for stream in control.incoming() {
                    let Ok(mut stream) = stream else { break };
                    let mut command = [0_u8; 1];
                    while stream.read_exact(&mut command).is_ok() {
                        let answer = match command[0] {
                            COMMAND_CHECK_READY => {
                                checks.fetch_add(1, Ordering::SeqCst);
                                let refused = refusals
                                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                                        left.checked_sub(1)
                                    })
                                    .is_ok();
                                vec![if refused { 1 } else { ACK_SUCCESS }]
                            }
                            COMMAND_GET_AUDIO_CONFIG => {
                                configs.fetch_add(1, Ordering::SeqCst);
                                let mut answer = vec![ACK_SUCCESS];
                                answer.extend_from_slice(&rate.to_ne_bytes());
                                answer.push(channels);
                                answer
                            }
                            COMMAND_START => {
                                starts.fetch_add(1, Ordering::SeqCst);
                                vec![ACK_SUCCESS]
                            }
                            _ => vec![ACK_SUCCESS],
                        };
                        if stream.write_all(&answer).is_err() {
                            break;
                        }
                    }
                }
            });
            let data_bytes = Arc::clone(&stack.data_bytes);
            std::thread::spawn(move || {
                for stream in data.incoming() {
                    let Ok(mut stream) = stream else { break };
                    let mut buffer = [0_u8; 4096];
                    while let Ok(read) = stream.read(&mut buffer) {
                        if read == 0 {
                            break;
                        }
                        data_bytes.fetch_add(read, Ordering::SeqCst);
                    }
                }
            });
            stack
        }

        fn checks(&self) -> usize {
            self.checks.load(Ordering::SeqCst)
        }
    }

    /// A connect answers as soon as the link is up, and the audio channel
    /// over it is negotiated afterwards. The Play that waited for the
    /// headphones lands in that gap, so the stack's first "not ready" is
    /// asked again rather than believed, and the stream starts at the rate
    /// the stack names once it is ready.
    #[test]
    fn a_sink_that_is_not_ready_at_play_is_asked_again_until_it_is() {
        let stack = FakeStack::listen("late", 2, 44_100, 2);
        let audio = Audio::simulated_with(stack.sockets.clone(), Duration::from_secs(3));
        let _ = audio.load(Source::File(fixture("chapters.mp3")));
        settled(&audio);
        let _ = audio.play();
        assert!(wait_for(
            || audio.sink_evidence().is_some(),
            Duration::from_secs(5)
        ));
        assert_eq!(
            audio.sink_evidence().map(|evidence| evidence.connected),
            Some(true)
        );
        assert!(stack.checks() >= 3, "{} attempts", stack.checks());
        assert_eq!(stack.starts.load(Ordering::SeqCst), 1);
        assert_eq!(stack.configs.load(Ordering::SeqCst), 1);
        assert!(wait_for(
            || stack.data_bytes.load(Ordering::SeqCst) > 0,
            Duration::from_secs(2)
        ));
        assert!(wait_for(
            || matches!(audio.state(), DeviceResult::Audio { position_ms, .. } if position_ms > 0),
            Duration::from_secs(3)
        ));
        assert!(audio.is_playing());
    }

    /// The patience is finite. A stack that never becomes ready is the
    /// headphones being gone, and the book is paused where it is.
    #[test]
    fn a_sink_that_stays_not_ready_is_given_up_on_when_the_patience_runs_out() {
        let stack = FakeStack::listen("never", usize::MAX, 48_000, 2);
        let patience = Duration::from_millis(700);
        let audio = Audio::simulated_with(stack.sockets.clone(), patience);
        let _ = audio.load(Source::File(fixture("chapters.mp3")));
        settled(&audio);
        let pressed = Instant::now();
        let _ = audio.play();
        assert!(
            audio.is_playing(),
            "the press is taken while the sink is awaited"
        );
        assert!(wait_for(
            || audio.sink_evidence().is_some(),
            Duration::from_secs(5)
        ));
        assert!(pressed.elapsed() >= patience);
        assert_eq!(
            audio.sink_evidence().map(|evidence| evidence.connected),
            Some(false)
        );
        assert_eq!(
            audio.state(),
            DeviceResult::Failed(DeviceError::Unreachable)
        );
        assert!(stack.checks() >= 2, "{} attempts", stack.checks());
        assert_eq!(stack.starts.load(Ordering::SeqCst), 0);
        assert!(!audio.is_playing());
    }

    /// No A2DP control socket exists on a host, which is exactly the state
    /// of a reader with no headphones connected. Pressing Play then proves
    /// the headphones are gone, once the simulated player's short patience
    /// is spent, and keeps the book and its place so the next Play, with
    /// headphones, carries on from here.
    #[test]
    fn playing_without_headphones_keeps_the_place_and_says_so() {
        assert!(!std::path::Path::new(super::CONTROL_SOCKET).exists());
        let audio = Audio::simulated();
        let _ = audio.load(Source::File(fixture("chapters.mp3")));
        settled(&audio);
        let _ = audio.seek(2_000);
        let _ = audio.play();
        let deadline = Instant::now() + Duration::from_secs(5);
        while audio.sink_evidence().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            audio.sink_evidence().map(|evidence| evidence.connected),
            Some(false)
        );
        assert_eq!(
            audio.state(),
            DeviceResult::Failed(DeviceError::Unreachable)
        );
        assert!(!audio.is_playing());
        // The place survives: a pause clears the complaint and the position
        // is still where the seek put it.
        let DeviceResult::Audio {
            state, position_ms, ..
        } = audio.pause()
        else {
            panic!("the book is still loaded");
        };
        assert_eq!(state, AudioPlaybackState::Paused);
        assert!((1_500..=2_100).contains(&position_ms), "{position_ms}");
    }

    #[test]
    fn speed_is_kept_within_what_sounds_acceptable() {
        let audio = Audio::simulated();
        let _ = audio.set_speed(175);
        assert_eq!(audio.speed_percent(), 175);
        let _ = audio.set_speed(900);
        assert_eq!(audio.speed_percent(), 300);
        let _ = audio.set_speed(1);
        assert_eq!(audio.speed_percent(), 50);
    }

    #[test]
    fn a_media_fault_is_not_evidence_the_headphones_left() {
        // A fault before anything is written says nothing about the sink.
        let state = std::sync::Mutex::new(super::State::default());
        super::record_sink(&state, true);
        let before = super::locked(&state).sink;
        super::pause_with(&state, DeviceError::Backend);
        assert_eq!(super::locked(&state).sink, before);
    }

    #[test]
    fn a_failed_sink_write_is_evidence_the_headphones_left() {
        let state = std::sync::Mutex::new(super::State::default());
        super::record_sink(&state, true);
        let written = super::sink_write(&state, Err(DeviceError::Unreachable));
        assert!(written.is_err());
        assert_eq!(
            super::locked(&state)
                .sink
                .map(|evidence| evidence.connected),
            Some(false)
        );
    }

    #[test]
    fn a_repeated_answer_is_still_a_new_observation() {
        let state = std::sync::Mutex::new(super::State::default());
        super::record_sink(&state, false);
        let first = super::locked(&state).sink.expect("evidence");
        super::record_sink(&state, false);
        let second = super::locked(&state).sink.expect("evidence");
        assert_eq!(first.connected, second.connected);
        assert_ne!(first.observation, second.observation);
    }

    #[test]
    fn a_worker_can_start_and_shut_down_without_hardware() {
        let audio = Audio::simulated();
        drop(audio);
    }
}
