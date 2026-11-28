//! Media: voice, camera, screen capture and the picture pipeline.

#[cfg(feature = "gui")]
pub mod audio;
pub mod bitrate;
#[cfg(feature = "gui")]
pub mod camera;
pub mod codec;
#[cfg(feature = "gui")]
pub mod cursor;
#[cfg(feature = "gui")]
pub mod hw_h264;
#[cfg(feature = "gui")]
pub mod loopback;
#[cfg(feature = "gui")]
pub mod openh264_runtime;
#[cfg(feature = "gui")]
pub mod player;
#[cfg(feature = "gui")]
pub mod processing;
#[cfg(feature = "gui")]
pub mod screen;
#[cfg(feature = "gui")]
pub mod temporal;
#[cfg(feature = "gui")]
pub mod video;
#[cfg(feature = "gui")]
pub mod voice;
#[cfg(all(feature = "gui", windows))]
pub mod vulkan_h264;
#[cfg(all(feature = "gui", windows))]
pub mod wgc;

#[cfg(all(test, windows))]
mod bench;
