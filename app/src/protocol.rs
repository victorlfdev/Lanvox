//! The wire language between a client and a server, and how packets are framed.
//!
//! Every packet is a 4-byte little-endian length followed by a bincode body.

use std::io::{self, Read, Write};

use bincode::Options;
use serde::{Deserialize, Serialize};

use crate::model::*;

pub const DEFAULT_PORT: u16 = 47800;
pub const DISCOVERY_PORT: u16 = 47801;
// Kept from the previous name on purpose; see branding.rs.
pub const DISCOVERY_MAGIC: &str = "HUDDLE5";
/// Largest packet accepted, to guard against garbage on the socket.
pub const MAX_PACKET: usize = 32 * 1024 * 1024;
pub const MAX_VIDEO_SIDE: u32 = 8192;
pub const MAX_VIDEO_PIXELS: u64 = 3840 * 2160;
pub const MAX_TILES: usize = 4096;
pub const MAX_TILE_BYTES: usize = 1024 * 1024;

pub fn picture_bytes(width: u32, height: u32) -> Option<usize> {
    let pixels = u64::from(width) * u64::from(height);
    if width == 0
        || height == 0
        || width > MAX_VIDEO_SIDE
        || height > MAX_VIDEO_SIDE
        || pixels > MAX_VIDEO_PIXELS
    {
        return None;
    }
    usize::try_from(pixels.checked_mul(4)?).ok()
}

/// One compressed rectangle of a picture.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tile {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub jpeg: Vec<u8>,
}

/// One picture update of a stream: either a full picture (keyframe) or only the
/// tiles that changed since the previous one.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VideoFrame {
    pub seq: u32,
    pub width: u32,
    pub height: u32,
    pub keyframe: bool,
    pub tiles: Vec<Tile>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EncodedVideoFrame {
    pub seq: u32,
    pub width: u32,
    pub height: u32,
    pub keyframe: bool,
    pub data: Vec<u8>,
}

/// Largest pointer bitmap accepted, per side.
pub const MAX_CURSOR_SIDE: u16 = 128;

/// The bitmap of a mouse pointer (straight, non-premultiplied RGBA).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CursorShape {
    pub id: u32,
    pub w: u16,
    pub h: u16,
    pub hot_x: u16,
    pub hot_y: u16,
    pub rgba: Vec<u8>,
}

impl CursorShape {
    pub fn valid(&self) -> bool {
        self.w > 0
            && self.h > 0
            && self.w <= MAX_CURSOR_SIDE
            && self.h <= MAX_CURSOR_SIDE
            && self.hot_x < self.w
            && self.hot_y < self.h
            && self.rgba.len() == usize::from(self.w) * usize::from(self.h) * 4
    }
}

/// Where the sharer's pointer is, sent apart from the video so the viewer can draw it
/// at its own refresh rate instead of at the video frame rate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CursorState {
    /// Hotspot position as a fraction (0..=65535) of the shared area.
    pub x: u16,
    pub y: u16,
    pub visible: bool,
    /// Width of the shared area in desktop pixels, to size the pointer like the sharer sees it.
    pub area_w: u32,
    pub shape_id: u32,
    /// Attached when the shape changed, and repeated now and then so late viewers and
    /// lost packets recover.
    pub shape: Option<CursorShape>,
}

impl CursorState {
    pub fn valid(&self) -> bool {
        self.area_w > 0
            && self.area_w <= MAX_VIDEO_SIDE
            && self.shape.as_ref().is_none_or(CursorShape::valid)
    }
}

impl VideoFrame {
    /// Bound allocations and reject out-of-bounds or overlapping rectangles before decoding.
    pub fn validate(&self) -> bool {
        if picture_bytes(self.width, self.height).is_none()
            || self.tiles.is_empty()
            || self.tiles.len() > MAX_TILES
        {
            return false;
        }
        let mut area = 0u64;
        let mut positions = std::collections::HashSet::new();
        for tile in &self.tiles {
            let (x, y, w, h) = (
                u32::from(tile.x),
                u32::from(tile.y),
                u32::from(tile.w),
                u32::from(tile.h),
            );
            if w == 0
                || h == 0
                || x + w > self.width
                || y + h > self.height
                || tile.jpeg.is_empty()
                || tile.jpeg.len() > MAX_TILE_BYTES
            {
                return false;
            }
            // Camera frames are a single rectangle; desktop deltas use a 64px grid.
            if self.tiles.len() > 1
                && (x % 64 != 0
                    || y % 64 != 0
                    || w != 64.min(self.width - x)
                    || h != 64.min(self.height - y))
            {
                return false;
            }
            if !positions.insert((x, y)) {
                return false;
            }
            area += u64::from(w) * u64::from(h);
        }
        (!self.keyframe || area == u64::from(self.width) * u64::from(self.height))
            && self.byte_size() <= MAX_PACKET - 64
    }

    pub fn byte_size(&self) -> usize {
        self.tiles.iter().map(|t| t.jpeg.len() + 8).sum::<usize>() + 16
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    Authenticate {
        hello: Box<ClientMsg>,
        token: String,
    },
    Hello {
        name: String,
        color: u32,
        public_key: [u8; 32],
        signature: Vec<u8>,
    },
    SendMessage {
        channel: ChannelId,
        text: String,
    },
    CreateGuild {
        name: String,
    },
    CreateChannel {
        guild: GuildId,
        name: String,
        kind: ChannelKind,
    },
    /// Ask someone (by name) to be your friend; also accepts if they already asked you.
    FriendRequest {
        name: String,
    },
    /// Accept or decline a request, cancel your own request, or remove a friend (accept = false).
    FriendRespond {
        user: UserId,
        accept: bool,
    },
    /// Start a private call with one or more friends.
    StartCall {
        members: Vec<UserId>,
    },
    InviteToCall {
        call: ChannelId,
        user: UserId,
    },
    DeclineCall {
        call: ChannelId,
    },
    Typing {
        channel: ChannelId,
    },
    JoinVoice {
        channel: ChannelId,
    },
    LeaveVoice,
    SetVoiceFlags {
        muted: bool,
        deafened: bool,
    },
    /// 20 ms of mono 24 kHz audio.
    Audio {
        seq: u32,
        pcm: Vec<i16>,
    },
    AudioEncoded {
        seq: u32,
        sent_micros: u64,
        opus: Vec<u8>,
    },
    StreamAudio {
        seq: u32,
        sent_micros: u64,
        packet: Vec<u8>,
    },
    Speaking(bool),
    StartStream(StreamInfo),
    StopStream(StreamKind),
    Frame {
        kind: StreamKind,
        frame: VideoFrame,
    },
    EncodedFrame {
        kind: StreamKind,
        frame: EncodedVideoFrame,
    },
    /// The pointer of the screen being shared.
    Cursor(CursorState),
    Watch {
        owner: UserId,
        kind: StreamKind,
    },
    Unwatch {
        owner: UserId,
        kind: StreamKind,
    },
    RequestKeyframe {
        owner: UserId,
        kind: StreamKind,
    },
    Ping(u64),
    Workspace(WorkspaceCommand),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome {
        you: UserId,
        snapshot: ServerSnapshot,
    },
    UserUpdated(User),
    Message(Message),
    ChannelCreated(Channel),
    GuildCreated(Guild),
    /// Your whole friend list, sent whenever it changes.
    Friends(Vec<FriendEntry>),
    /// A call you are part of was created or its members changed.
    CallUpdated(CallInfo),
    CallEnded {
        call: ChannelId,
    },
    Typing {
        user: UserId,
        channel: ChannelId,
    },
    Voice {
        user: UserId,
        state: VoiceState,
    },
    Audio {
        from: UserId,
        seq: u32,
        pcm: Vec<i16>,
    },
    AudioEncoded {
        from: UserId,
        seq: u32,
        sent_micros: u64,
        opus: Vec<u8>,
    },
    StreamAudio {
        from: UserId,
        seq: u32,
        sent_micros: u64,
        packet: Vec<u8>,
    },
    MediaSession {
        port: u16,
        token: [u8; 16],
        key: [u8; 32],
    },
    Speaking {
        user: UserId,
        speaking: bool,
    },
    StreamStarted(StreamInfo),
    StreamStopped {
        owner: UserId,
        kind: StreamKind,
    },
    Frame {
        owner: UserId,
        kind: StreamKind,
        frame: VideoFrame,
    },
    EncodedFrame {
        owner: UserId,
        kind: StreamKind,
        frame: EncodedVideoFrame,
    },
    Cursor {
        owner: UserId,
        state: CursorState,
    },
    /// Someone started watching your stream (or lost pictures): send a full picture.
    KeyframeWanted {
        kind: StreamKind,
    },
    Viewers {
        kind: StreamKind,
        count: u32,
    },
    Pong(u64),
    Error(String),
    Challenge {
        version: u32,
        nonce: [u8; 32],
    },
    Workspace(WorkspaceEvent),
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn write_packet<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let body =
        bincode::serialize(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    w.write_all(&(body.len() as u32).to_le_bytes())?;
    w.write_all(&body)?;
    Ok(())
}

pub fn encode_packet<T: Serialize>(msg: &T) -> Vec<u8> {
    let body = bincode::serialize(msg).unwrap_or_default();
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

pub fn read_packet<R: Read, T: for<'de> Deserialize<'de>>(r: &mut R) -> io::Result<T> {
    read_packet_limit(r, MAX_PACKET)
}

pub fn read_packet_limit<R: Read, T: for<'de> Deserialize<'de>>(
    r: &mut R,
    limit: usize,
) -> io::Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "packet too large",
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(limit as u64)
        .reject_trailing_bytes()
        .deserialize(&body)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_oversized_packets_trailing_bytes_and_forged_lengths() {
        assert!(read_packet::<_, ClientMsg>(
            &mut ((MAX_PACKET as u32 + 1).to_le_bytes().as_slice())
        )
        .is_err());
        let mut packet = encode_packet(&ClientMsg::Ping(42));
        packet.push(0);
        let size = packet.len() as u32 - 4;
        packet[..4].copy_from_slice(&size.to_le_bytes());
        assert!(read_packet::<_, ClientMsg>(&mut packet.as_slice()).is_err());
        // A tiny body cannot ask the deserializer to allocate an enormous vector.
        let mut forged = 8u32.to_le_bytes().to_vec();
        forged.extend_from_slice(&u64::MAX.to_le_bytes());
        assert!(read_packet::<_, Vec<u8>>(&mut forged.as_slice()).is_err());
        assert!(matches!(
            read_packet::<_, ClientMsg>(&mut encode_packet(&ClientMsg::Ping(7)).as_slice())
                .unwrap(),
            ClientMsg::Ping(7)
        ));
    }

    #[test]
    fn rejects_invalid_dimensions_rectangles_and_incomplete_keyframes() {
        assert!(picture_bytes(u32::MAX, u32::MAX).is_none());
        assert!(picture_bytes(0, 64).is_none());
        let mut frame = VideoFrame {
            seq: 1,
            width: 128,
            height: 64,
            keyframe: true,
            tiles: vec![Tile {
                x: 0,
                y: 0,
                w: 64,
                h: 64,
                jpeg: vec![1],
            }],
        };
        assert!(!frame.validate());
        frame.tiles.push(Tile {
            x: 64,
            ..frame.tiles[0].clone()
        });
        assert!(frame.validate());
        frame.tiles[1].x = 0;
        assert!(!frame.validate());
        frame.tiles[1].x = u16::MAX;
        assert!(!frame.validate());
        frame.tiles[1].x = 64;
        frame.tiles[1].w = 0;
        assert!(!frame.validate());
    }
}
