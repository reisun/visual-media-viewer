#![allow(dead_code)]

#[path = "../../src/video_player.rs"]
mod video_player;

#[cfg(test)]
mod tests {
    use super::video_player::{PlaybackState, VideoPlayer};
    use std::path::PathBuf;
    use std::thread;
    use std::time::{Duration, Instant};

    fn open(name: &str) -> VideoPlayer {
        let dir = std::env::var_os("VMV_VIDEO_FIXTURES").expect("run scripts/test-video.sh");
        VideoPlayer::open(&PathBuf::from(dir).join(name), false).expect("open fixture")
    }

    fn first_frame(player: &mut VideoPlayer, target: f64) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(frame) = player.poll_frame(0.0) {
                assert_eq!((frame.width, frame.height), (160, 96));
                assert_eq!(frame.rgba.len(), 160 * 96 * 4);
                assert!(
                    frame.pts >= target - 0.05,
                    "stale frame {} < {target}",
                    frame.pts
                );
                assert!(
                    frame.pts <= target + 0.15,
                    "late frame {} > {target}",
                    frame.pts
                );
                return;
            }
            assert!(Instant::now() < deadline, "no frame after seek to {target}");
            assert!(
                !matches!(player.state, PlaybackState::Finished),
                "ended without frame at target {target}: {}",
                player.diag_info
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn repeated_seek_restarts_at_requested_frame_across_formats() {
        for name in ["silent.mp4", "audio.mp4", "bframes.mov"] {
            let mut player = open(name);
            assert!((player.duration - 4.0).abs() < 0.1);
            first_frame(&mut player, 0.0);
            for target in [2.0, 0.5, 3.0, 0.0, 1.5, 2.5, 0.0] {
                let previous_revision = player.frame_revision();
                assert!(player.seek(target).is_ok());
                assert!(player.current_frame().is_none(), "old frame survives seek");
                first_frame(&mut player, target);
                assert!(player.frame_revision() > previous_revision);
            }
            player.stop();
        }
    }

    #[test]
    fn paused_frames_do_not_request_another_upload_and_seek_recovers() {
        let mut player = open("silent.mp4");
        first_frame(&mut player, 0.0);
        player.toggle_pause();
        let revision = player.frame_revision();
        let pts = player.current_frame().unwrap().pts;
        for _ in 0..20 {
            assert!(player.poll_frame_for_upload(0.0, Some(revision)).is_none());
            assert_eq!(player.frame_revision(), revision);
            assert_eq!(player.current_frame().unwrap().pts, pts);
            thread::sleep(Duration::from_millis(2));
        }
        assert!(player.seek(2.0).is_ok());
        first_frame(&mut player, 2.0);
        assert!(player.frame_revision() > revision);
    }

    #[test]
    fn eof_can_restart_with_the_retained_input() {
        let mut player = open("silent.mp4");
        assert!(player.seek(2.0).is_ok());
        first_frame(&mut player, 2.0);
        let deadline = Instant::now() + Duration::from_secs(8);
        while !matches!(player.state, PlaybackState::Finished) {
            player.poll_frame(0.0);
            assert!(Instant::now() < deadline, "EOF never reached");
            thread::sleep(Duration::from_millis(2));
        }
        let revision = player.frame_revision();
        player.toggle_pause();
        first_frame(&mut player, 0.0);
        assert!(player.frame_revision() > revision);
    }

    #[test]
    fn delayed_eof_frames_match_full_reopen_behavior() {
        // The existing EOF drain drops frames if no scaler was created before EOF.
        // Preserve observable behavior here without changing the delicate decoder path.
        fn outcome(name: &str, target: f64, disable_reuse: bool) -> Option<f64> {
            if disable_reuse {
                std::env::set_var("VMV_DISABLE_INPUT_REUSE", "1");
            } else {
                std::env::remove_var("VMV_DISABLE_INPUT_REUSE");
            }
            let mut player = open(name);
            assert!(player.seek(target).is_ok());
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                if let Some(frame) = player.poll_frame(0.0) {
                    return Some(frame.pts);
                }
                if matches!(player.state, PlaybackState::Finished) {
                    return None;
                }
                assert!(Instant::now() < deadline, "EOF case hung");
                thread::sleep(Duration::from_millis(2));
            }
        }

        let original = std::env::var_os("VMV_DISABLE_INPUT_REUSE");
        for (name, target) in [
            ("short.mp4", 0.0),
            ("silent.mp4", 3.75),
            ("bframes.mov", 3.75),
        ] {
            let reopened = outcome(name, target, true);
            let reused = outcome(name, target, false);
            match (reopened, reused) {
                (None, None) => {}
                (Some(old), Some(new)) => assert!((old - new).abs() < 0.05),
                _ => panic!(
                    "EOF behavior changed for {name} at {target}: {reopened:?} -> {reused:?}"
                ),
            }
        }
        match original {
            Some(value) => std::env::set_var("VMV_DISABLE_INPUT_REUSE", value),
            None => std::env::remove_var("VMV_DISABLE_INPUT_REUSE"),
        }
    }
}
