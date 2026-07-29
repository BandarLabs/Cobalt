# Audiobook Studio

Type a topic and the application asks Exa for deep research, OpenAI for an
original spoken script, and ElevenLabs for narration. It packages the bounded
MP3 parts as a Kobo `.mp3z` in `/mnt/onboard/Audiobooks`, so the result remains
available in My Books.

The finished result opens in `kobo_sdk::audio::AudioPlayer` with deterministic
album art, position, ±30-second seek, play/pause and software volume. Playback
uses a connected Bluetooth audio-class device. If none is connected, Play
opens the component's own headphones/speaker picker; after pairing and
connection the pending audiobook starts automatically.

Provider keys remain runtime secrets named `exa`, `openai` and `elevenlabs`.
They are attached only to the exact provider endpoints allowed by `kobod` and
are never sent to, stored by, or logged from the application process.
