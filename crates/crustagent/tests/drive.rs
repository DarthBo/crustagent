//! Drive an Agent through its lifecycle against a real character (skips if absent).

use crustagent::{Agent, AudioSink};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn merlin() -> Option<Agent> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/agents/Merlin.acs");
    Agent::load(path).ok()
}

/// Advance the agent by `ms`, in 16ms steps.
fn run(agent: &mut Agent, ms: u32) {
    let mut left = ms;
    while left > 0 {
        let dt = left.min(16);
        agent.update(dt);
        left -= dt;
    }
}

#[test]
fn show_speak_move_hide() {
    let Some(mut agent) = merlin() else {
        eprintln!("no Merlin.acs — skipping");
        return;
    };

    // Starts hidden, nothing to draw.
    assert!(!agent.is_visible());
    assert!(agent.composite_current().is_none());

    // Show -> becomes visible and has something to draw; then idles.
    agent.show();
    run(&mut agent, 3000);
    assert!(agent.is_visible());
    assert!(agent.composite_current().is_some());
    assert!(agent.balloon().is_none());

    // Speak -> balloon appears and reveals words over time.
    agent.speak("hello there my friend");
    agent.update(16);
    let b0 = agent.balloon().expect("balloon while speaking");
    assert_eq!(b0.total_words, 4);
    assert!(b0.shown_words >= 1);
    run(&mut agent, 700); // ~2 more words paced in
    let b1 = agent.balloon().expect("still speaking");
    assert!(b1.shown_words > b0.shown_words, "words should reveal over time");
    // After the phrase finishes speaking, the balloon lingers (auto-hide) fully revealed,
    // then clears — while the character resumes idling.
    run(&mut agent, 2000);
    let done = agent.balloon().expect("balloon lingers after speech");
    assert_eq!(done.shown_words, done.total_words);
    assert!(agent.is_visible());
    run(&mut agent, 3500); // past the auto-hide linger
    assert!(agent.balloon().is_none());

    // Move -> position ends at the destination.
    agent.set_position(0, 0);
    agent.move_to(400, 250, 300);
    run(&mut agent, 5000);
    assert_eq!(agent.position(), (400, 250));

    // Hide -> becomes invisible and stops drawing.
    agent.hide();
    run(&mut agent, 3000);
    assert!(!agent.is_visible());
    assert!(agent.composite_current().is_none());
}

#[test]
fn gesture_and_stop() {
    let Some(mut agent) = merlin() else { return };
    agent.show();
    run(&mut agent, 2000);

    // A known gesture composites while playing.
    agent.play("Greet");
    run(&mut agent, 100);
    assert!(agent.composite_current().is_some());

    // stop() clears the queue; the agent falls back to idling while visible.
    agent.speak("this should be cleared");
    agent.stop();
    run(&mut agent, 3000);
    assert!(agent.is_visible());
}

#[test]
fn play_looping_holds_until_stopped() {
    let Some(mut agent) = merlin() else { return };
    agent.show();
    run(&mut agent, 3000);

    // A looping gesture keeps playing well past a single cycle — a one-shot `play` would
    // have returned to idle by now.
    agent.play_looping("Greet");
    run(&mut agent, 6000);
    assert!(agent.is_gesturing(), "looping gesture should still be playing");

    // stop() ends the loop; the agent falls back to idling while visible.
    agent.stop();
    run(&mut agent, 3000);
    assert!(!agent.is_gesturing(), "stop() should end the loop");
    assert!(agent.is_visible());
}

#[test]
fn play_looping_yields_to_the_next_request() {
    let Some(mut agent) = merlin() else { return };
    agent.show();
    run(&mut agent, 3000);

    agent.play_looping("Greet");
    run(&mut agent, 1000);
    assert!(agent.is_gesturing());

    // Queuing another request preempts the loop rather than being blocked behind it forever.
    agent.set_position(0, 0);
    agent.move_to(300, 200, 300);
    run(&mut agent, 5000);
    assert_eq!(agent.position(), (300, 200), "the queued move ran");
}

#[test]
fn say_over_reveals_while_gesturing() {
    let Some(mut agent) = merlin() else { return };
    agent.show();
    run(&mut agent, 3000);

    agent.play_looping("Greet");
    run(&mut agent, 200);
    assert!(agent.is_gesturing());

    // Talk *over* the running gesture: the balloon reveals and the gesture keeps playing.
    agent.say_over("one two three four");
    agent.update(16);
    let b = agent.balloon().expect("overlay balloon while gesturing");
    assert!(b.shown_words >= 1);
    assert!(agent.is_gesturing(), "gesture continues during say_over");

    run(&mut agent, 1200);
    let b2 = agent.balloon().expect("still revealing");
    assert!(b2.shown_words > b.shown_words, "overlay reveals over time");
    assert!(agent.is_gesturing(), "still gesturing after the overlay reveal");
}

#[test]
fn fires_embedded_sound_effects() {
    let Some(mut agent) = merlin() else { return };

    // Find an animation whose *first* frame carries a sound (deterministic: frame 0 always
    // plays), so we can assert the sink is driven.
    let anim = agent.file().animations.iter().enumerate().find_map(|(i, a)| {
        a.frames
            .first()
            .is_some_and(|f| f.sound_ndx >= 0)
            .then(|| agent.file().gesture_names[i].clone())
    });
    let Some(anim) = anim else {
        eprintln!("no frame-0 sound animation in Merlin — skipping");
        return;
    };

    struct Counter(Arc<AtomicUsize>);
    impl AudioSink for Counter {
        fn play(&mut self, _wav: &[u8]) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let count = Arc::new(AtomicUsize::new(0));
    agent.set_audio_sink(Box::new(Counter(count.clone())));

    agent.show();
    run(&mut agent, 2000);
    agent.play(anim.clone());
    run(&mut agent, 500);

    assert!(
        count.load(Ordering::SeqCst) > 0,
        "no sound effect fired for {anim}"
    );
}

#[test]
fn exit_branching_gesture_plays_its_return() {
    let Some(mut agent) = merlin() else { return };
    agent.show();
    run(&mut agent, 3000);

    // Merlin's "Pleased" is a returnType==1 (ExitBranching) gesture: forward it is the
    // hands-together motion (~500ms), and the return is the exit walk back out. With the
    // return played, the whole gesture runs noticeably longer than the forward half.
    assert!(agent.file().animation("Pleased").is_some());
    agent.play("Pleased");
    agent.update(16); // start the gesture
    assert!(agent.is_gesturing());

    // Measure how long the gesture itself runs (before the idle rest pause).
    let mut elapsed = 16u32;
    while agent.is_gesturing() && elapsed < 5000 {
        agent.update(16);
        elapsed += 16;
    }
    // Forward-only would end near ~500ms; with the exit return it runs ~900ms.
    assert!(
        elapsed > 700,
        "Pleased ended after only {elapsed}ms — exit return not played"
    );
}

#[test]
fn emits_lifecycle_and_request_events() {
    use crustagent::Event;
    let Some(mut agent) = merlin() else { return };

    let show = agent.show();
    let mut events = Vec::new();
    run_collect(&mut agent, 3000, &mut events);

    // The show request starts and completes, and the character reports becoming visible.
    assert!(events.contains(&Event::RequestStarted(show)));
    assert!(events.contains(&Event::RequestCompleted(show)));
    assert!(events.contains(&Event::Shown));
    // Draining an idle queue eventually reports idling.
    assert!(events.contains(&Event::IdleStarted));

    // A speak request raises balloon + speech events, ended by SpeechEnded.
    events.clear();
    agent.speak("one two three");
    run_collect(&mut agent, 2000, &mut events);
    assert!(events.contains(&Event::BalloonShown));
    assert!(events.contains(&Event::SpeechStarted));
    assert!(events.contains(&Event::SpeechEnded));
    assert!(events.iter().any(|e| matches!(e, Event::IdleEnded)));
}

#[test]
fn fires_bookmarks_in_order() {
    use crustagent::Event;
    let Some(mut agent) = merlin() else { return };
    agent.show();
    run(&mut agent, 2000);

    let mut events = Vec::new();
    agent.speak(r"first \Mrk=10\ second \Mrk=20\ third");
    run_collect(&mut agent, 3000, &mut events);

    let marks: Vec<i64> = events
        .iter()
        .filter_map(|e| match e {
            Event::Bookmark(n) => Some(*n),
            _ => None,
        })
        .collect();
    assert_eq!(marks, vec![10, 20], "bookmarks should fire in order");
}

#[test]
fn think_shows_a_thought_balloon_without_speech() {
    use crustagent::{BalloonKind, Event};
    let Some(mut agent) = merlin() else { return };
    agent.show();
    run(&mut agent, 2000);

    let mut events = Vec::new();
    agent.think("pondering deeply");
    agent.update(16);
    let b = agent.balloon().expect("think balloon");
    assert_eq!(b.kind, BalloonKind::Think);

    run_collect(&mut agent, 3000, &mut events);
    // A think raises balloon events but no speech events.
    assert!(events.contains(&Event::BalloonShown));
    assert!(!events.contains(&Event::SpeechStarted));
}

#[test]
fn pause_freezes_word_reveal() {
    let Some(mut agent) = merlin() else { return };
    agent.show();
    run(&mut agent, 2000);
    agent.speak("alpha bravo charlie delta echo");
    run(&mut agent, 350);
    let before = agent.balloon().expect("speaking").shown_words;

    agent.pause();
    assert!(agent.is_paused());
    run(&mut agent, 2000); // time passes, but frozen
    assert_eq!(agent.balloon().expect("still shown").shown_words, before);

    agent.resume();
    run(&mut agent, 1000);
    assert!(agent.balloon().map(|b| b.shown_words).unwrap_or(99) >= before);
}

/// Advance in 16ms steps, collecting drained events.
fn run_collect(agent: &mut Agent, ms: u32, out: &mut Vec<crustagent::Event>) {
    let mut left = ms;
    while left > 0 {
        let dt = left.min(16);
        agent.update(dt);
        out.extend(agent.drain_events());
        left -= dt;
    }
}
