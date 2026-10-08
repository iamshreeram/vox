/// Suppresses the ambient listener's own TTS output from being re-transcribed
/// as if a human said it. Uses a fake speaking signal until the TTS manager exists.
pub struct EchoSuppressor;
