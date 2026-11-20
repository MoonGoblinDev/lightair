//! lightair-engine's tests against real libobs, one test program (one build and link, not one per file).
//! libobs runs once per process: a test that starts an engine runs in a process of its
//! own (`lightair_engine::testing`).

mod audio;
mod audio_block;
mod audio_engine;
mod camera_studio;
mod device_channels;
mod encoding_options;
mod import_obs;
mod input_options;
mod media;
mod mix_tracks;
mod monitor;
mod multi_recording;
mod outputs;
mod phone_camera;
mod plugins;
mod portable_kinds;
mod reconcile;
mod recording_tools;
mod smoke;
mod stems;
mod stream_protocols;
mod stream_sharing;
mod studio_take;
mod timers;
mod transition;
