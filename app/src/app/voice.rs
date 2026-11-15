use super::*;

impl LanvoxApp {
    /// Whether the server says I am out of voice, ignoring the window while a
    /// join to `pending.1` is still in flight. The pending join is cleared only
    /// once the server reports me in that very channel (or it times out): being
    /// reported in the *previous* channel must not clear it, or the "left"
    /// broadcast that follows would tear the new call's audio down.
    pub(crate) fn resolve_out_of_call(
        pending: &mut Option<(Instant, ChannelId)>,
        mine: Option<ChannelId>,
        now: Instant,
    ) -> bool {
        match *pending {
            Some((_, target)) if mine == Some(target) => {
                *pending = None;
                false
            }
            Some((at, _)) if now.saturating_duration_since(at) < Duration::from_secs(10) => false,
            Some(_) => {
                *pending = None;
                mine.is_none()
            }
            None => mine.is_none(),
        }
    }

    /// True while a join is sent but not yet confirmed or timed out.
    pub(crate) fn joining_voice(&self) -> bool {
        self.voice_join
            .is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(10))
    }

    pub fn join_voice(&mut self, channel: ChannelId) {
        if self.view.as_ref().map(|v| v.my_voice().channel) == Some(Some(channel)) {
            return;
        }
        if self
            .view
            .as_ref()
            .and_then(|v| v.my_voice().channel)
            .is_some()
        {
            self.stop_media();
        }
        self.send(ClientMsg::JoinVoice { channel });
        self.voice_join = Some((Instant::now(), channel));
        if !self.demo {
            self.start_audio();
        }
        let muted = self.audio_shared.muted.load(Ordering::Relaxed);
        let deafened = self.audio_shared.deafened.load(Ordering::Relaxed);
        self.send(ClientMsg::SetVoiceFlags { muted, deafened });
    }

    pub fn start_audio(&mut self) {
        self.mixer.clear();
        self.apply_local_audio_preferences();
        self.audio = Some(AudioEngine::start(
            self.audio_shared.clone(),
            self.mixer.clone(),
            self.settings.input_device.clone(),
            self.settings.output_device.clone(),
        ));
    }

    /// Local volume and mute for one person; saved values are applied on first use.
    pub fn person_audio(&self, id: UserId, name: &str) -> (f32, bool) {
        if self.is_blocked(id) {
            self.mixer.set_user(id, 0.0, true);
            return (0.0, true);
        }
        if !self.mixer.has_user(id) {
            if let Some(&(v, m)) = self
                .settings
                .user_audio
                .get(&self.person_audio_key(id, name))
            {
                self.mixer.set_user(id, v, m);
            }
        }
        self.mixer.user(id)
    }

    pub fn set_person_audio(&mut self, id: UserId, name: &str, volume: f32, muted: bool) {
        self.mixer
            .set_user(id, volume, muted || self.is_blocked(id));
        let key = self.person_audio_key(id, name);
        if (volume - 1.0).abs() < 0.005 && !muted {
            self.settings.user_audio.remove(&key);
        } else {
            self.settings.user_audio.insert(key, (volume, muted));
        }
    }

    /// Local volume and mute for one person's screen-share audio.
    pub fn stream_audio(&self, id: UserId, name: &str) -> (f32, bool) {
        if self.is_blocked(id) {
            self.mixer.set_stream_user(id, 0.0, true);
            return (0.0, true);
        }
        if !self.mixer.has_stream_user(id) {
            if let Some(&(v, m)) = self
                .settings
                .stream_audio
                .get(&self.person_audio_key(id, name))
            {
                self.mixer.set_stream_user(id, v, m);
            }
        }
        self.mixer.stream_user(id)
    }

    pub fn set_stream_audio(&mut self, id: UserId, name: &str, volume: f32, muted: bool) {
        self.mixer
            .set_stream_user(id, volume, muted || self.is_blocked(id));
        let key = self.person_audio_key(id, name);
        if (volume - 1.0).abs() < 0.005 && !muted {
            self.settings.stream_audio.remove(&key);
        } else {
            self.settings.stream_audio.insert(key, (volume, muted));
        }
    }

    /// Restart audio with new devices if a call or mic test is running.
    pub fn restart_audio(&mut self) {
        if self.audio.is_some() {
            self.audio = None;
            self.start_audio();
        }
    }

    pub(super) fn stop_media(&mut self) {
        if self.share.take().is_some() {
            self.send(ClientMsg::StopStream(StreamKind::Screen));
        }
        if self.camera.take().is_some() {
            self.send(ClientMsg::StopStream(StreamKind::Camera));
        }
        let me = self.me();
        self.pictures.remove((me, StreamKind::Screen));
        self.pictures.remove((me, StreamKind::Camera));
        for key in self.watching.drain() {
            if let Some(c) = &self.client {
                c.send(ClientMsg::Unwatch {
                    owner: key.0,
                    kind: key.1,
                });
            }
        }
        self.unwatched.clear();
        self.focused = None;
        self.fullscreen = false;
        self.popout = None;
        if !self.audio_shared.mic_test.load(Ordering::Relaxed) {
            self.audio = None;
        }
        self.mixer.clear();
    }

    /// Calls other people started that are ringing for me.
    pub fn ringing(&self) -> Vec<CallInfo> {
        let Some(v) = &self.view else {
            return Vec::new();
        };
        let mine = v.my_voice().channel;
        let mut list: Vec<CallInfo> = v
            .calls
            .values()
            .filter(|c| {
                c.members.contains(&v.me)
                    && c.host != v.me
                    && !self.is_blocked(c.host)
                    && !self.dismissed_calls.contains(&c.id)
                    && mine != Some(c.id)
            })
            .cloned()
            .collect();
        list.sort_by_key(|c| c.id);
        list
    }

    pub fn join_call(&mut self, id: ChannelId) {
        // A call that belongs to a conversation opens inside that conversation.
        self.selected = Some(
            self.view
                .as_ref()
                .and_then(|v| v.conversation_of_call(id))
                .unwrap_or(id),
        );
        self.join_voice(id);
    }

    pub fn decline_call(&mut self, id: ChannelId) {
        self.dismissed_calls.insert(id);
        self.send(ClientMsg::DeclineCall { call: id });
    }

    pub fn start_call(&mut self, members: Vec<UserId>) {
        if members.is_empty() {
            return;
        }
        self.send(ClientMsg::StartCall { members });
        self.group_pick.clear();
    }

    pub fn leave_voice(&mut self) {
        if let Some(ch) = self.view.as_ref().and_then(|v| v.my_voice().channel) {
            self.dismissed_calls.insert(ch);
        }
        self.voice_join = None;
        self.stop_media();
        self.send(ClientMsg::LeaveVoice);
    }

    pub fn toggle_mute(&mut self) {
        let sh = &self.audio_shared;
        let was_silenced = sh.muted.load(Ordering::Relaxed) || sh.deafened.load(Ordering::Relaxed);
        crate::sfx::play(
            if was_silenced {
                crate::sfx::Sfx::Unmute
            } else {
                crate::sfx::Sfx::Mute
            },
            &self.settings,
        );
        let deaf = sh.deafened.load(Ordering::Relaxed);
        if deaf {
            // Un-muting while deafened also un-deafens.
            sh.deafened.store(false, Ordering::Relaxed);
            sh.muted.store(false, Ordering::Relaxed);
        } else {
            sh.muted
                .store(!sh.muted.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        self.sync_voice_flags();
    }

    pub fn toggle_deafen(&mut self) {
        let sh = &self.audio_shared;
        crate::sfx::play(
            if sh.deafened.load(Ordering::Relaxed) {
                crate::sfx::Sfx::Undeafen
            } else {
                crate::sfx::Sfx::Deafen
            },
            &self.settings,
        );
        sh.deafened
            .store(!sh.deafened.load(Ordering::Relaxed), Ordering::Relaxed);
        self.sync_voice_flags();
    }

    pub(super) fn sync_voice_flags(&mut self) {
        let muted = self.audio_shared.muted.load(Ordering::Relaxed);
        let deafened = self.audio_shared.deafened.load(Ordering::Relaxed);
        self.mixer.deafened.store(deafened, Ordering::Relaxed);
        if deafened {
            self.mixer.clear();
        }
        if self.demo {
            if let Some(v) = &mut self.view {
                let me = v.me;
                let s = v.voice.entry(me).or_default();
                s.muted = muted || deafened;
                s.deafened = deafened;
            }
        }
        self.send(ClientMsg::SetVoiceFlags { muted, deafened });
    }

    // ----- camera -----
}
