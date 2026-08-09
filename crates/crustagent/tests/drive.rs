// SPDX-License-Identifier: MIT OR Apache-2.0

//! Drive an Agent through its lifecycle against a real character (skips if absent).

use crustagent::{Agent, AudioSink};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Load `Merlin.acs` from `assets/agents`, wherever under it the file has been filed.
fn merlin() -> Option<Agent> {
    fn find(dir: &std::path::Path) -> Option<PathBuf> {
        let mut sub = Vec::new();
        for path in std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()) {
            if path.is_dir() {
                sub.push(path);
            } else if path
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("Merlin.acs"))
            {
                return Some(path);
            }
        }
        sub.iter().find_map(|d| find(d))
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/agents");
    Agent::load(find(&root)?).ok()
}

/// A minimal in-memory character with SHOWING/HIDING/idle animations but **no** `MOVING*`
/// state — so a move has to teleport rather than walk.
fn teleporter() -> Agent {
    use crustagent::format::{
        AcsFile, Animation, FileHeader, Frame, FrameImage, Guid, Name, ReturnKind, Rgba, State,
    };
    let frame = |dur: u16| Frame {
        duration: dur,
        sound_ndx: -1,
        exit_frame: -1,
        branching: Vec::new(),
        images: vec![FrameImage {
            image_ndx: 0,
            offset: (0, 0),
        }],
        overlays: Vec::new(),
    };
    let anim = |name: &str, dur: u16| Animation {
        name: name.into(),
        return_kind: ReturnKind::None,
        return_name: String::new(),
        frames: vec![frame(dur)],
    };
    let animations = vec![anim("Show", 20), anim("Hide", 20), anim("Idle", 100)];
    let gesture_names: Vec<String> = animations.iter().map(|a| a.name.clone()).collect();
    let states = vec![
        State {
            name: "SHOWING".into(),
            animations: vec!["Show".into()],
        },
        State {
            name: "HIDING".into(),
            animations: vec!["Hide".into()],
        },
        State {
            name: "IDLINGLEVEL1".into(),
            animations: vec!["Idle".into()],
        },
        // deliberately no MOVINGLEFT/RIGHT/UP/DOWN
    ];
    let header = FileHeader {
        version_major: 2,
        version_minor: 0,
        guid: Guid([0; 16]),
        image_size: (1, 1),
        transparency: 0,
        style: 0,
        palette: Vec::new(),
    };
    let names = vec![Name {
        language: 0x0409,
        name: "Tele".into(),
        desc1: String::new(),
        desc2: String::new(),
    }];
    let images = vec![Rgba {
        width: 1,
        height: 1,
        pixels: vec![0, 0, 0, 0],
    }];
    Agent::from_file(AcsFile::from_parts_rgba(
        header,
        None,
        None,
        names,
        states,
        gesture_names,
        animations,
        images,
        Vec::new(),
    ))
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
    assert!(
        b1.shown_words > b0.shown_words,
        "words should reveal over time"
    );
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
    assert!(
        agent.is_gesturing(),
        "looping gesture should still be playing"
    );

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
    assert!(
        agent.is_gesturing(),
        "still gesturing after the overlay reveal"
    );
}

#[test]
fn move_without_a_walk_animation_teleports() {
    let mut agent = teleporter();
    agent.show();
    run(&mut agent, 3000);
    agent.set_position(0, 0);
    assert_eq!(agent.position(), (0, 0));

    agent.move_to(300, 200, 300);
    // Still vanishing (HIDING is ~200ms) — a glide at 300px/s would already have crept
    // forward by now, but a teleport holds the start position until the jump.
    run(&mut agent, 60);
    assert_eq!(
        agent.position(),
        (0, 0),
        "teleport holds position while vanishing (no glide)"
    );

    run(&mut agent, 3000); // HIDING → jump → SHOWING completes
    assert_eq!(
        agent.position(),
        (300, 200),
        "teleport lands exactly on the destination"
    );
}

#[test]
fn play_looping_honors_the_built_in_loop_point() {
    use crustagent::format::{
        AcsFile, Animation, Branch, FileHeader, Frame, FrameImage, Guid, Name, ReturnKind, Rgba,
        State,
    };
    // Animation "Loop": frame 0 is a one-time intro; frames 1-2 are the loop (frame 2 -> 1).
    let f = |img: u32, branch: &[(i16, u16)]| Frame {
        duration: 10,
        sound_ndx: -1,
        exit_frame: -1,
        branching: branch
            .iter()
            .map(|&(frame_ndx, probability)| Branch {
                frame_ndx,
                probability,
            })
            .collect(),
        images: vec![FrameImage {
            image_ndx: img,
            offset: (0, 0),
        }],
        overlays: Vec::new(),
    };
    let loop_anim = Animation {
        name: "Loop".into(),
        return_kind: ReturnKind::None,
        return_name: String::new(),
        frames: vec![f(0, &[]), f(1, &[]), f(2, &[(1, 100)])],
    };
    let header = FileHeader {
        version_major: 2,
        version_minor: 0,
        guid: Guid([0; 16]),
        image_size: (1, 1),
        transparency: 0,
        style: 0,
        palette: Vec::new(),
    };
    let images = (0..3)
        .map(|_| Rgba {
            width: 1,
            height: 1,
            pixels: vec![0, 0, 0, 0],
        })
        .collect();
    let mut agent = Agent::from_file(AcsFile::from_parts_rgba(
        header,
        None,
        None,
        vec![Name {
            language: 0x0409,
            name: "Loop".into(),
            desc1: String::new(),
            desc2: String::new(),
        }],
        vec![State {
            name: "IDLINGLEVEL1".into(),
            animations: vec!["Loop".into()],
        }],
        vec!["Loop".into()],
        vec![loop_anim],
        images,
        Vec::new(),
    ));
    agent.show_fast();
    agent.update(16);
    agent.play_looping("Loop");

    // The intro frame (0) lasts 100ms; run past one full pass, then confirm the body keeps
    // cycling and the intro never replays.
    run(&mut agent, 400);
    let mut saw_body = false;
    for _ in 0..40 {
        run(&mut agent, 16);
        if let Some((_, frame, _)) = agent.current_frame_token() {
            assert_ne!(frame, 0, "intro frame replayed — loop point not honored");
            saw_body |= frame == 1 || frame == 2;
        }
    }
    assert!(saw_body, "the loop body should be cycling");
}

#[test]
fn wait_holds_the_current_frame_instead_of_replaying() {
    use crustagent::format::{
        AcsFile, Animation, FileHeader, Frame, FrameImage, Guid, Name, ReturnKind, Rgba, State,
    };
    // A 3-frame gesture (no branch, no return).
    let f = |img: u32| Frame {
        duration: 10,
        sound_ndx: -1,
        exit_frame: -1,
        branching: Vec::new(),
        images: vec![FrameImage {
            image_ndx: img,
            offset: (0, 0),
        }],
        overlays: Vec::new(),
    };
    let wag = Animation {
        name: "Wag".into(),
        return_kind: ReturnKind::None,
        return_name: String::new(),
        frames: vec![f(0), f(1), f(2)],
    };
    let header = FileHeader {
        version_major: 2,
        version_minor: 0,
        guid: Guid([0; 16]),
        image_size: (1, 1),
        transparency: 0,
        style: 0,
        palette: Vec::new(),
    };
    let images = (0..3)
        .map(|_| Rgba {
            width: 1,
            height: 1,
            pixels: vec![0, 0, 0, 0],
        })
        .collect();
    let mut agent = Agent::from_file(AcsFile::from_parts_rgba(
        header,
        None,
        None,
        vec![Name {
            language: 0x0409,
            name: "Wag".into(),
            desc1: String::new(),
            desc2: String::new(),
        }],
        vec![State {
            name: "IDLINGLEVEL1".into(),
            animations: vec!["Wag".into()],
        }],
        vec!["Wag".into()],
        vec![wag],
        images,
        Vec::new(),
    ));
    agent.show_fast();
    agent.update(16);

    // Play the 3-frame gesture (300ms), then a long wait. During the wait the frame must
    // hold — the regression replayed the gesture's frames over the wait's timeline.
    agent.play("Wag");
    agent.wait(2000);
    run(&mut agent, 400); // ~100ms into the wait (gesture is 300ms)
    let held = agent.current_frame_token();
    let mut changed = false;
    for _ in 0..60 {
        agent.update(16);
        if agent.current_frame_token() != held {
            changed = true;
            break;
        }
    }
    assert!(
        !changed,
        "frame changed during the wait — it replayed instead of holding"
    );
}

#[test]
fn fires_embedded_sound_effects() {
    let Some(mut agent) = merlin() else { return };

    // Find an animation whose *first* frame carries a sound (deterministic: frame 0 always
    // plays), so we can assert the sink is driven.
    let anim = agent
        .file()
        .animations
        .iter()
        .enumerate()
        .find_map(|(i, a)| {
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

#[test]
fn speaks_one_of_several_alternatives() {
    let Some(mut agent) = merlin() else { return };
    agent.show();
    run(&mut agent, 2000);

    // `a|b|c` offers alternatives; the agent speaks exactly one of them.
    agent.speak("alpha|bravo charlie|delta echo foxtrot");
    agent.update(16);
    let b = agent.balloon().expect("balloon while speaking");
    assert!(
        [1, 2, 3].contains(&b.total_words),
        "expected one alternative (1, 2 or 3 words), got {}",
        b.total_words
    );
}

// -- interactive balloons (questions with clickable choices) ------------------------------

use crustagent::{AskHit, BalloonMode, BalloonUi, Button, ButtonSet, Event};

/// The demo question: two choices, one check box, and a Cancel button.
fn question() -> BalloonUi {
    BalloonUi::new("Select one of these things:")
        .heading("What would you like to do?")
        .choice("Write a letter")
        .choice("Make a chart")
        .checkbox("Don't ask again")
        .buttons(ButtonSet::Cancel)
}

/// A shown, idling agent built from the synthetic character (no asset needed).
fn asking_agent() -> Agent {
    let mut agent = teleporter();
    agent.show();
    run(&mut agent, 500);
    agent
}

#[test]
fn modal_question_holds_the_queue_until_answered() {
    let mut agent = asking_agent();
    agent.ask(question());
    let queued = agent.play("Idle");
    run(&mut agent, 500);

    // The question is up, and the request behind it has not started.
    let view = agent.balloon().expect("balloon while asking");
    let ask = view.ask.expect("interactive balloon");
    assert_eq!(ask.buttons, vec![Button::Cancel]);
    assert!(agent.pending_ask().is_some());
    let mut events = Vec::new();
    run_collect(&mut agent, 500, &mut events);
    assert!(
        !events.contains(&Event::RequestStarted(queued)),
        "a modal question must hold the queue: {events:?}"
    );

    // Clicking a choice answers with its 1-based index and takes the balloon down.
    agent.report_ask_hit(AskHit::Choice(1));
    let answered: Vec<Event> = agent
        .drain_events()
        .into_iter()
        .filter(|e| matches!(e, Event::Answered { .. }))
        .collect();
    assert_eq!(
        answered,
        vec![Event::Answered {
            choice: Some(2),
            button: None,
            checked: 0,
            text: None,
        }]
    );
    assert!(agent.pending_ask().is_none());
    assert!(agent.balloon().is_none());

    // ...and the queue moves on.
    let mut after = Vec::new();
    run_collect(&mut agent, 500, &mut after);
    assert!(
        after.contains(&Event::RequestStarted(queued)),
        "the queue should resume once answered: {after:?}"
    );
}

#[test]
fn check_boxes_toggle_and_ride_along_with_the_answer() {
    let mut agent = asking_agent();
    agent.ask(question());
    run(&mut agent, 100);
    let _ = agent.drain_events();

    // A check box toggles in place — no answer, balloon stays up.
    agent.report_ask_hit(AskHit::CheckBox(0));
    assert_eq!(agent.ask_checked(), 0b1);
    assert!(agent.balloon().is_some());
    assert!(!agent
        .drain_events()
        .iter()
        .any(|e| matches!(e, Event::Answered { .. })));

    // It shows its state in the laid-out rows, and toggles back off.
    let lines = agent.balloon().unwrap().layout.lines;
    assert!(
        lines.iter().any(|l| l.contains("[x] Don't ask again")),
        "ticked box should render: {lines:?}"
    );
    agent.report_ask_hit(AskHit::CheckBox(0));
    assert_eq!(agent.ask_checked(), 0);
    agent.report_ask_hit(AskHit::CheckBox(0));

    // The commit button is what carries the check-box state out.
    agent.report_ask_hit(AskHit::Button(Button::Cancel));
    let answered = agent
        .drain_events()
        .into_iter()
        .find(|e| matches!(e, Event::Answered { .. }));
    assert_eq!(
        answered,
        Some(Event::Answered {
            choice: None,
            button: Some(Button::Cancel),
            checked: 0b1,
            text: None,
        })
    );
}

#[test]
fn a_question_never_auto_hides() {
    let mut agent = asking_agent();
    agent.ask(question());
    run(&mut agent, 30_000); // far past the 3s speech auto-hide
    assert!(
        agent.balloon().and_then(|b| b.ask).is_some(),
        "a question waits for its answer"
    );
}

#[test]
fn modeless_question_lingers_without_holding_the_queue() {
    let mut agent = asking_agent();
    agent.ask(question().mode(BalloonMode::Modeless));
    let queued = agent.play("Idle");
    let mut events = Vec::new();
    run_collect(&mut agent, 500, &mut events);

    assert!(
        events.contains(&Event::RequestStarted(queued)),
        "a modeless question must not hold the queue: {events:?}"
    );
    assert!(
        agent.pending_ask().is_some(),
        "...but the balloon lingers, awaiting an answer"
    );
}

#[test]
fn auto_down_dismisses_on_a_click_without_answering() {
    let mut agent = asking_agent();
    agent.ask(question().mode(BalloonMode::AutoDown));
    run(&mut agent, 100);
    let _ = agent.drain_events();

    agent.report_click(crustagent::MouseButton::Left, 0, 0);
    assert!(agent.pending_ask().is_none());
    assert!(
        !agent
            .drain_events()
            .iter()
            .any(|e| matches!(e, Event::Answered { .. })),
        "an unanswered dismissal raises no Answered"
    );
}

#[test]
fn stop_releases_an_unanswered_modal_question() {
    let mut agent = asking_agent();
    agent.ask(question());
    let queued = agent.play("Idle");
    run(&mut agent, 200);

    agent.stop(); // cancels `queued` and drops the question
    assert!(agent.pending_ask().is_none());
    let mut events = Vec::new();
    run_collect(&mut agent, 500, &mut events);
    assert!(
        events.contains(&Event::RequestCompleted(queued)),
        "stop() cancels the queue behind the question: {events:?}"
    );
    // The agent is free again — it goes back to idling rather than waiting forever.
    assert!(agent.is_idle());
}

#[test]
fn hits_are_ignored_when_no_question_is_showing() {
    let mut agent = asking_agent();
    agent.report_ask_hit(AskHit::Choice(0));
    agent.report_ask_hit(AskHit::Button(Button::Ok));
    assert!(!agent
        .drain_events()
        .iter()
        .any(|e| matches!(e, Event::Answered { .. })));
}

// -- the balloon's text field ------------------------------------------------------------

use crustagent::AskEdit;

/// A search-style question: a text field and Search/Close.
fn search_question() -> BalloonUi {
    BalloonUi::new("What would you like to do?")
        .input("Type your question here")
        .buttons(ButtonSet::SearchClose)
}

#[test]
fn typing_fills_the_field_and_rides_out_with_the_answer() {
    let mut agent = asking_agent();
    agent.ask(search_question());
    run(&mut agent, 100);
    let _ = agent.drain_events();
    assert!(agent.ask_has_input());

    agent.report_ask_text("mail merge");
    assert_eq!(agent.ask_text(), "mail merge");
    assert_eq!(agent.ask_caret(), 10);

    // The balloon shows what was typed rather than the placeholder.
    let view = agent.balloon().unwrap().ask.unwrap().input.unwrap();
    assert_eq!(view.value, "mail merge");
    assert!(!view.shows_prompt(false));

    // Enter submits with the set's first button — Search, as Office's search balloon did.
    agent.report_ask_submit();
    let answered = agent
        .drain_events()
        .into_iter()
        .find(|e| matches!(e, crustagent::Event::Answered { .. }));
    assert_eq!(
        answered,
        Some(crustagent::Event::Answered {
            choice: None,
            button: Some(crustagent::Button::Search),
            checked: 0,
            text: Some("mail merge".to_string()),
        })
    );
}

#[test]
fn editing_keys_move_the_caret_and_delete_around_it() {
    let mut agent = asking_agent();
    agent.ask(search_question());
    run(&mut agent, 100);

    agent.report_ask_text("mail merge");
    agent.report_ask_edit(AskEdit::Home);
    assert_eq!(agent.ask_caret(), 0);
    agent.report_ask_edit(AskEdit::Backspace); // at the start: nothing to delete
    assert_eq!(agent.ask_text(), "mail merge");

    agent.report_ask_edit(AskEdit::Delete);
    assert_eq!(agent.ask_text(), "ail merge");

    agent.report_ask_edit(AskEdit::End);
    agent.report_ask_edit(AskEdit::Backspace);
    assert_eq!(agent.ask_text(), "ail merg");

    // Insertion happens at the caret, not the end.
    agent.report_ask_edit(AskEdit::Home);
    agent.report_ask_edit(AskEdit::Right);
    agent.report_ask_text("XY");
    assert_eq!(agent.ask_text(), "aXYil merg");

    agent.report_ask_edit(AskEdit::Clear);
    assert_eq!(agent.ask_text(), "");
    assert_eq!(agent.ask_caret(), 0);
}

#[test]
fn editing_a_multibyte_value_never_splits_a_char() {
    let mut agent = asking_agent();
    agent.ask(search_question());
    run(&mut agent, 100);

    // Accents and an emoji: every one is multi-byte, so a byte-indexed caret would panic.
    agent.report_ask_text("héllo 🌍");
    assert_eq!(agent.ask_caret(), 7);
    agent.report_ask_edit(AskEdit::Backspace);
    assert_eq!(agent.ask_text(), "héllo ");
    agent.report_ask_edit(AskEdit::Home);
    agent.report_ask_edit(AskEdit::Right);
    agent.report_ask_text("é");
    assert_eq!(agent.ask_text(), "hééllo ");
}

#[test]
fn control_characters_never_reach_the_field() {
    let mut agent = asking_agent();
    agent.ask(search_question());
    run(&mut agent, 100);

    // Enter and Tab are the host's to interpret; they must not land in the buffer.
    agent.report_ask_text("a\nb\tc\r");
    assert_eq!(agent.ask_text(), "abc");
}

#[test]
fn the_caret_can_be_placed_by_a_click_and_is_clamped() {
    let mut agent = asking_agent();
    agent.ask(search_question());
    run(&mut agent, 100);

    agent.report_ask_text("hello");
    agent.report_ask_caret(2);
    assert_eq!(agent.ask_caret(), 2);
    agent.report_ask_caret(999);
    assert_eq!(agent.ask_caret(), 5);
}

#[test]
fn a_question_without_a_field_ignores_text_entirely() {
    let mut agent = asking_agent();
    agent.ask(question()); // choices + check box, no field
    run(&mut agent, 100);
    let _ = agent.drain_events();

    assert!(!agent.ask_has_input());
    agent.report_ask_text("ignored");
    agent.report_ask_edit(AskEdit::Backspace);
    agent.report_ask_submit();
    assert_eq!(agent.ask_text(), "");
    assert!(agent.pending_ask().is_some(), "submit must not answer it");
    assert!(!agent
        .drain_events()
        .iter()
        .any(|e| matches!(e, crustagent::Event::Answered { .. })));

    // ...and its answer carries no text.
    agent.report_ask_hit(AskHit::Choice(0));
    let answered = agent
        .drain_events()
        .into_iter()
        .find(|e| matches!(e, crustagent::Event::Answered { .. }));
    assert!(matches!(
        answered,
        Some(crustagent::Event::Answered { text: None, .. })
    ));
}

#[test]
fn a_prefilled_field_starts_populated_with_the_caret_at_the_end() {
    let mut agent = asking_agent();
    agent.ask(BalloonUi::new("Search for:").input_with("Search", "Resume"));
    run(&mut agent, 100);
    assert_eq!(agent.ask_text(), "Resume");
    assert_eq!(agent.ask_caret(), 6);
}

#[test]
fn clicking_the_field_does_not_answer_the_question() {
    let mut agent = asking_agent();
    agent.ask(search_question());
    run(&mut agent, 100);
    let _ = agent.drain_events();

    agent.report_ask_hit(AskHit::Input);
    assert!(agent.pending_ask().is_some());
    assert!(!agent
        .drain_events()
        .iter()
        .any(|e| matches!(e, crustagent::Event::Answered { .. })));
}
