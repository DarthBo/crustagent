//! Drive an Agent through its lifecycle against a real character (skips if absent).

use crustagent::Agent;
use std::path::PathBuf;

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
    // After the phrase finishes, balloon clears and it idles again.
    run(&mut agent, 2000);
    assert!(agent.balloon().is_none());
    assert!(agent.is_visible());

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
    assert!(!agent.is_idle());

    let mut elapsed = 16u32;
    while !agent.is_idle() && elapsed < 5000 {
        agent.update(16);
        elapsed += 16;
    }
    // Forward-only would end near ~500ms; with the exit return it runs ~900ms.
    assert!(
        elapsed > 700,
        "Pleased ended after only {elapsed}ms — exit return not played"
    );
}
