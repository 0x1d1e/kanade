use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

// ---- a niri that remembers what it is told ----

struct Fake {
    outputs: RefCell<Vec<Output>>,
    commands: RefCell<Vec<(String, Vec<String>)>>,
    written: RefCell<Vec<(String, Vec<Edit>)>>,
    refuse: RefCell<Option<String>>,
    cannot_write: Cell<bool>,
    cannot_undo: Cell<bool>,
    cannot_ask: Cell<bool>,
    clock: Cell<u64>,
    unix: Cell<u64>,

    // every `run`, refused ones included, which `commands` cannot show
    runs: Cell<u32>,

    // when true, `run` returns `Unanswered` instead of carrying out the command
    hangs: Cell<bool>,

    // whether the pending file existed at each command
    file: PathBuf,
    file_at_commands: RefCell<Vec<bool>>,

    // when the fake's clock follows real time from here, for a test that runs the trial's own
    // clock thread: nothing else moves it then
    real: RefCell<Option<(std::time::Instant, u64)>>,
}

impl Fake {
    fn new(outputs: Vec<Output>, file: &Path) -> Fake {
        Fake {
            outputs: RefCell::new(outputs),
            commands: RefCell::default(),
            written: RefCell::default(),
            refuse: RefCell::default(),
            cannot_write: Cell::new(false),
            cannot_undo: Cell::new(false),
            cannot_ask: Cell::new(false),
            clock: Cell::new(1_000),
            unix: Cell::new(1_700_000_000_000),
            runs: Cell::new(0),
            hangs: Cell::new(false),
            file: file.to_owned(),
            file_at_commands: RefCell::default(),
            real: RefCell::default(),
        }
    }

    fn at(&self, millis: u64) {
        self.clock.set(millis);
        self.unix.set(self.unix.get() + millis);
    }

    // the clock runs with real time from where it is, as `Real` does, so a test can run the
    // trial's own clock thread against it
    fn follow_real_time(&self) {
        *self.real.borrow_mut() = Some((std::time::Instant::now(), self.clock.get()));
    }

    fn state(&self, name: &str) -> Output {
        self.outputs
            .borrow()
            .iter()
            .find(|output| output.name == name)
            .cloned()
            .unwrap()
    }
}

impl Port for Fake {
    fn outputs(&self) -> Result<Vec<Output>, String> {
        if self.cannot_ask.get() {
            return Err(String::from("niri is not there"));
        }

        Ok(self.outputs.borrow().clone())
    }

    fn run(&self, name: &str, args: &[String]) -> Result<(), RunError> {
        self.runs.set(self.runs.get() + 1);
        self.file_at_commands.borrow_mut().push(self.file.exists());

        // niri's CLI reads a negative number as a flag, unless `--` comes before it
        let dashes = args.iter().position(|arg| arg == "--");

        for (at, arg) in args.iter().enumerate() {
            if arg.starts_with('-')
                && arg.len() > 1
                && arg.parse::<f64>().is_ok()
                && !dashes.is_some_and(|dashes| dashes < at)
            {
                return Err(RunError::Refused(format!(
                    "error: unexpected argument '{arg}' found"
                )));
            }
        }

        if self.hangs.get() {
            return Err(RunError::Unanswered(String::from(
                "niri msg output timed out after 2000 ms",
            )));
        }

        if let Some(why) = self.refuse.borrow().clone() {
            return Err(RunError::Refused(why));
        }

        self.commands
            .borrow_mut()
            .push((name.to_owned(), args.to_vec()));

        let mut outputs = self.outputs.borrow_mut();
        let output = outputs
            .iter_mut()
            .find(|output| output.name == name)
            .unwrap();
        let word = |at: usize| args.get(at).map(String::as_str).unwrap_or_default();

        match word(0) {
            "off" => output.logical = None,
            "on" => {
                output.logical.get_or_insert(Logical {
                    x: 0,
                    y: 0,
                    scale: 1.0,
                    transform: Transform::Normal,
                });
                output.current = Some(0);
            }
            "mode" => output.current = output.modes.iter().position(|m| m.text() == word(1)),
            "scale" => output.logical.as_mut().unwrap().scale = word(1).parse().unwrap(),
            "transform" => {
                output.logical.as_mut().unwrap().transform = Transform::from_name(word(1)).unwrap();
            }
            "position" => {
                let logical = output.logical.as_mut().unwrap();

                assert_eq!((word(1), word(2)), ("set", "--"));
                logical.x = word(3).parse().unwrap();
                logical.y = word(4).parse().unwrap();
            }
            "vrr" => output.vrr_enabled = args.last().map(String::as_str) == Some("on"),
            other => panic!("unknown command {other}"),
        }

        Ok(())
    }

    fn write(&self, output: &Output, edits: &[&Edit]) -> Result<(String, Option<String>), String> {
        if self.cannot_write.get() {
            return Err(String::from("niri refuses the config"));
        }

        self.written.borrow_mut().push((
            output.name.clone(),
            edits.iter().map(|edit| (*edit).clone()).collect(),
        ));

        Ok((String::from("written"), Some(String::from("after"))))
    }

    fn undo_write(&self) -> Result<String, String> {
        if self.cannot_undo.get() {
            Err(String::from("config changed since write"))
        } else {
            Ok(String::from("undone"))
        }
    }

    fn now(&self) -> u64 {
        match &*self.real.borrow() {
            Some((start, from)) => from + start.elapsed().as_millis() as u64,
            None => self.clock.get(),
        }
    }

    fn unix_ms(&self) -> u64 {
        self.unix.get()
    }
}

fn mode(width: u16, height: u16, refresh: u32, preferred: bool) -> Mode {
    Mode {
        width,
        height,
        refresh,
        preferred,
    }
}

fn panel() -> Output {
    Output {
        name: String::from("eDP-1"),
        make: Some(String::from("AU Optronics")),
        model: Some(String::from("0x7EAD")),
        serial: Some(String::from("0x00007EAD")),
        modes: vec![
            mode(1920, 1080, 144_001, true),
            mode(1920, 1080, 60_001, true),
        ],
        current: Some(0),
        vrr_supported: true,
        vrr_enabled: false,
        logical: Some(Logical {
            x: 0,
            y: 0,
            scale: 1.0,
            transform: Transform::Normal,
        }),
    }
}

fn dell() -> Output {
    Output {
        name: String::from("HDMI-A-5"),
        make: Some(String::from("Dell Inc.")),
        model: Some(String::from("DELL P1917S")),
        serial: Some(String::from("NGRP68260N2B")),
        modes: vec![
            mode(1280, 1024, 60_020, true),
            mode(1280, 1024, 75_025, false),
        ],
        current: Some(0),
        vrr_supported: false,
        vrr_enabled: false,
        logical: Some(Logical {
            x: 1920,
            y: 0,
            scale: 1.0,
            transform: Transform::Normal,
        }),
    }
}

// a directory for the pending file, removed at the end
struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        static COUNT: AtomicU32 = AtomicU32::new(0);

        let dir = std::env::temp_dir().join(format!(
            "kanade-outputs-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));

        fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join("outputs-pending.json")
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn setup() -> (Dir, Fake, Controller) {
    let dir = Dir::new();
    let fake = Fake::new(vec![panel(), dell()], &dir.file());
    let control = Controller::new(dir.file());

    (dir, fake, control)
}

#[test]
fn an_include_matched_by_edid_blocks_a_trial_before_the_live_edit() {
    let monitor = panel();
    let mut checked = None;
    let mut touched = false;
    let result = work::edit_if_editable(
        &monitor.name,
        Some(&Monitor {
            output: monitor.clone(),
            config: work::Config::default(),
        }),
        |id| {
            checked = Some(id.clone());
            assert_eq!(id, &monitor.id());
            Err(String::from("owned by mine.kdl"))
        },
        || {
            touched = true;
            Ok(())
        },
    );

    assert_eq!(result, Err(String::from("owned by mine.kdl")));
    assert_eq!(checked, Some(monitor.id()));
    assert!(!touched);
}

#[test]
fn connector_named_outputs_keep_their_editability_check() {
    let mut checked = None;
    let result = work::edit_if_editable(
        "DP-1",
        None,
        |id| {
            checked = Some(id.clone());
            assert_eq!(id, &crate::niri_config::OutputId::named("DP-1"));
            Ok(())
        },
        || Ok("trial started"),
    );

    assert_eq!(result, Ok("trial started"));
    assert_eq!(checked, Some(crate::niri_config::OutputId::named("DP-1")));
}

// ---- what niri says ----

#[test]
fn the_outputs_are_read_as_niri_prints_them() {
    let reply = crate::sources::json::parse(
        r#"{"Outputs":{"eDP-1":{"name":"eDP-1","make":"AU Optronics","model":"0x7EAD","serial":"0x00007EAD","physical_size":[340,190],"modes":[{"width":1920,"height":1080,"refresh_rate":144001,"is_preferred":true},{"width":1920,"height":1080,"refresh_rate":60001,"is_preferred":true}],"current_mode":0,"is_custom_mode":false,"vrr_supported":true,"vrr_enabled":false,"logical":{"x":0,"y":0,"width":1920,"height":1080,"scale":1.0,"transform":"Normal"}},
        "DP-2":{"name":"DP-2","make":"Unknown","model":"Unknown","serial":null,"physical_size":null,"modes":[{"width":800,"height":600,"refresh_rate":60000,"is_preferred":true}],"current_mode":null,"is_custom_mode":false,"vrr_supported":false,"vrr_enabled":false,"logical":null},
        "DP-3":{"name":"DP-3","make":"A","model":"B","serial":"1","modes":[],"current_mode":0,"is_custom_mode":true,"vrr_supported":false,"vrr_enabled":false,"logical":{"x":-5,"y":7,"width":1,"height":1,"scale":1.5,"transform":"Flipped270"}}}}"#,
    )
    .unwrap();
    let outputs = parse_outputs(&reply);

    assert_eq!(
        outputs.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(),
        ["DP-2", "DP-3", "eDP-1"]
    );
    assert_eq!(outputs[2], panel());

    // an output that is off, with nothing known of it
    assert!(!outputs[0].enabled());
    assert_eq!(outputs[0].make, None);
    assert_eq!(outputs[0].product(), "Unknown monitor");

    // a custom mode is no mode of the list; the position and turn are read as signed
    assert_eq!(outputs[1].current, None);
    assert_eq!(
        outputs[1].logical,
        Some(Logical {
            x: -5,
            y: 7,
            scale: 1.5,
            transform: Transform::Flipped270
        })
    );
}

#[test]
fn a_change_is_said_to_niri_and_written_to_the_config_in_niris_words() {
    let mode = mode(1920, 1080, 144_001, true);

    assert_eq!(Edit::Mode(mode).args(), ["mode", "1920x1080@144.001"]);
    assert_eq!(Edit::Scale(1.25).args(), ["scale", "1.25"]);
    assert_eq!(Edit::Scale(2.0).args(), ["scale", "2"]);
    assert_eq!(
        Edit::Transform(Transform::Flipped90).args(),
        ["transform", "flipped-90"]
    );
    assert_eq!(
        Edit::Position(-1920, 10).args(),
        ["position", "set", "--", "-1920", "10"]
    );
    assert_eq!(
        Edit::Vrr(Vrr::OnDemand).args(),
        ["vrr", "--on-demand", "on"]
    );
    assert_eq!(Edit::Vrr(Vrr::Off).args(), ["vrr", "off"]);
    assert_eq!(Edit::Enabled(false).args(), ["off"]);

    assert_eq!(
        Edit::Mode(mode).ops(),
        [(
            "mode",
            Op::Set(Value::Str(String::from("1920x1080@144.001")))
        )]
    );
    assert_eq!(
        Edit::Position(-1920, 10).ops(),
        [(
            "position",
            Op::Set(Value::Props(vec![
                (String::from("x"), Value::Int(-1920)),
                (String::from("y"), Value::Int(10)),
            ]))
        )]
    );
    assert_eq!(Edit::Scale(2.0).ops(), [("scale", Op::Set(Value::Int(2)))]);
    assert_eq!(
        Edit::Scale(1.5).ops(),
        [("scale", Op::Set(Value::Float(1.5)))]
    );
    assert_eq!(
        Edit::Vrr(Vrr::Off).ops(),
        [("variable-refresh-rate", Op::Remove)]
    );
    assert_eq!(Edit::Enabled(true).ops(), [("off", Op::Remove)]);
}

// ---- what may be tried ----

#[test]
fn only_what_the_output_can_do_is_tried() {
    let (panel, dell) = (panel(), dell());
    let both = [panel.clone(), dell.clone()];

    // a mode from its list, a scale in range, variable refresh rate where it is supported
    assert!(check(&Edit::Mode(panel.modes[1]), &panel, &both).is_ok());
    assert!(check(&Edit::Mode(mode(1920, 1080, 120_000, false)), &panel, &both).is_err());
    assert!(check(&Edit::Scale(0.5), &panel, &both).is_ok());
    assert!(check(&Edit::Scale(4.0), &panel, &both).is_ok());
    assert!(check(&Edit::Scale(0.49), &panel, &both).is_err());
    assert!(check(&Edit::Scale(4.01), &panel, &both).is_err());
    assert!(check(&Edit::Scale(f64::NAN), &panel, &both).is_err());
    assert!(check(&Edit::Vrr(Vrr::On), &panel, &both).is_ok());
    assert!(check(&Edit::Vrr(Vrr::On), &dell, &both).is_err());
    assert!(check(&Edit::Vrr(Vrr::Off), &dell, &both).is_ok());
}

#[test]
fn the_last_output_that_is_on_is_not_turned_off() {
    let (panel, dell) = (panel(), dell());
    let mut off = dell.clone();

    off.logical = None;

    // one of two is turned off, the other is then the last
    assert!(
        check(
            &Edit::Enabled(false),
            &panel,
            &[panel.clone(), dell.clone()]
        )
        .is_ok()
    );
    let why = check(&Edit::Enabled(false), &panel, &[panel.clone(), off.clone()]).unwrap_err();
    assert!(why.contains("only output"), "{why}");
    assert!(check(&Edit::Enabled(false), &panel, std::slice::from_ref(&panel)).is_err());

    // an output that is off is turned on, and nothing else is done to it
    assert!(check(&Edit::Enabled(true), &off, &[panel.clone(), off.clone()]).is_ok());
    assert!(check(&Edit::Scale(2.0), &off, &[panel, off.clone()]).is_err());
}

// ---- the trial ----

#[test]
fn the_pending_file_is_there_before_niri_is_told_and_gone_when_the_trial_ends() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(1.5), None)
        .unwrap();
    control
        .try_edit(&fake, "eDP-1", Edit::Transform(Transform::R90), None)
        .unwrap();

    // every command ran with the file already written
    assert_eq!(*fake.file_at_commands.borrow(), [true, true]);

    let file: Json = serde_json::from_str(&fs::read_to_string(dir.file()).unwrap()).unwrap();

    assert_eq!(file["output"], "eDP-1");
    assert_eq!(file["previous"]["scale"], 1.0);
    assert_eq!(file["previous"]["mode"], "1920x1080@144.001");
    assert_eq!(file["previous"]["transform"], "normal");

    // what the file holds is the state before the first change, not the last
    assert_eq!(fake.state("eDP-1").logical.unwrap().scale, 1.5);

    control.keep(&fake).unwrap();
    // keep no longer deletes the file; confirm_keep does
    assert!(dir.file().exists());
    control.confirm_keep();
    assert!(!dir.file().exists());
    assert!(control.session.is_none());
}

#[test]
fn nothing_is_done_when_the_pending_file_cannot_be_written() {
    let (dir, fake, _) = setup();

    // a file where the directory should be
    fs::write(dir.0.join("blocker"), b"").unwrap();

    let mut control = Controller::new(dir.0.join("blocker/outputs.json"));

    let why = control
        .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
        .unwrap_err();

    assert!(why.starts_with("Not tried"), "{why}");
    assert!(fake.commands.borrow().is_empty());
    assert!(control.session.is_none());
}

#[test]
fn a_change_niri_refuses_writes_nothing_and_ends_a_trial_it_began() {
    let (dir, fake, mut control) = setup();

    *fake.refuse.borrow_mut() = Some(String::from("scale is out of range for this output"));

    let why = control
        .try_edit(&fake, "eDP-1", Edit::Scale(3.0), None)
        .unwrap_err();

    assert_eq!(why, "scale is out of range for this output");
    assert!(control.session.is_none());
    assert!(!dir.file().exists());
    assert!(fake.written.borrow().is_empty());

    // refused after another change: that one stays tried
    *fake.refuse.borrow_mut() = None;
    control
        .try_edit(&fake, "eDP-1", Edit::Scale(1.5), None)
        .unwrap();
    *fake.refuse.borrow_mut() = Some(String::from("no"));
    assert!(
        control
            .try_edit(&fake, "eDP-1", Edit::Transform(Transform::R180), None)
            .is_err()
    );

    let session = control.session.as_ref().unwrap();

    assert_eq!(
        session.edits.keys().copied().collect::<Vec<_>>(),
        [Field::Scale]
    );
    assert!(dir.file().exists());
}

#[test]
fn a_trial_is_for_one_output_at_a_time() {
    let (_dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(1.5), None)
        .unwrap();

    let why = control
        .try_edit(&fake, "HDMI-A-5", Edit::Scale(1.5), None)
        .unwrap_err();

    assert!(why.contains("eDP-1"), "{why}");
    assert_eq!(fake.state("HDMI-A-5").logical.unwrap().scale, 1.0);
}

#[test]
fn each_edit_starts_the_fifteen_seconds_over() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(1.5), None)
        .unwrap();
    assert_eq!(control.remaining(1_000), Some(TRIAL_MS));

    fake.at(11_000);
    assert_eq!(control.remaining(11_000), Some(5_000));
    control
        .try_edit(&fake, "eDP-1", Edit::Transform(Transform::R90), None)
        .unwrap();
    assert_eq!(control.remaining(11_000), Some(TRIAL_MS));

    // the first deadline passes; the second has not
    fake.at(16_001);
    assert!(control.expire(&fake).is_none());
    assert!(dir.file().exists());

    fake.at(26_000);
    assert_eq!(control.expire(&fake), Some(Ok(Ended::Reverted)));
    assert!(!dir.file().exists());
}

#[test]
fn a_revert_puts_back_what_niri_showed_before_the_first_change() {
    let (dir, fake, mut control) = setup();
    let before = fake.state("eDP-1");

    for edit in [
        Edit::Scale(2.0),
        Edit::Transform(Transform::Flipped180),
        Edit::Position(100, -50),
        Edit::Mode(before.modes[1]),
        Edit::Vrr(Vrr::On),
    ] {
        control.try_edit(&fake, "eDP-1", edit, None).unwrap();
    }

    assert_ne!(fake.state("eDP-1"), before);
    assert_eq!(control.revert(&fake), Ok(Ended::Reverted));
    assert_eq!(fake.state("eDP-1"), before);
    assert!(!dir.file().exists());
    assert!(control.session.is_none());
    assert!(fake.written.borrow().is_empty());
}

#[test]
fn an_output_turned_off_is_turned_on_again_by_a_revert() {
    let (_dir, fake, mut control) = setup();
    let before = fake.state("HDMI-A-5");

    control
        .try_edit(&fake, "HDMI-A-5", Edit::Enabled(false), None)
        .unwrap();
    assert!(!fake.state("HDMI-A-5").enabled());

    control.revert(&fake).unwrap();

    // niri forgets where it was when it comes back; what it shows is put back
    assert_eq!(
        fake.state("HDMI-A-5").logical.map(|l| (l.x, l.y)),
        Some((1920, 0))
    );
    assert!(fake.state("HDMI-A-5").enabled());
    assert_eq!(fake.state("HDMI-A-5").current, before.current);
}

#[test]
fn a_revert_that_fails_keeps_the_file_and_tries_again() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
        .unwrap();
    *fake.refuse.borrow_mut() = Some(String::from("niri is busy"));
    fake.at(2_000);

    assert_eq!(control.revert(&fake), Err(String::from("niri is busy")));
    assert!(dir.file().exists());

    let session = control.session.as_ref().unwrap();

    assert_eq!(session.stuck.as_deref(), Some("niri is busy"));
    assert_eq!(session.retry_at, 2_000 + RETRY_MS);

    // the window to Keep is still the one the trial began with: a failed revert does not reopen it
    assert_eq!(session.deadline, 1_000 + TRIAL_MS);

    *fake.refuse.borrow_mut() = None;
    fake.at(7_000);

    assert_eq!(control.expire(&fake), Some(Ok(Ended::Reverted)));
    assert_eq!(fake.state("eDP-1"), panel());
    assert!(!dir.file().exists());
}

// a revert that failed while the page is gone waits out the retry deadline before it is tried
// again; the 250 ms tick must not hammer niri with a refused command
#[test]
fn a_failed_revert_waits_for_its_deadline_after_the_page_leaves() {
    let (_dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
        .unwrap();
    *fake.refuse.borrow_mut() = Some(String::from("niri is busy"));
    fake.at(2_000);

    let tried = fake.runs.get();

    assert_eq!(
        control.revert_when_due(&fake, true),
        Some(Err(String::from("niri is busy")))
    );
    assert_eq!(fake.runs.get(), tried + 1);

    let session = control.session.as_ref().unwrap();

    assert_eq!(session.stuck.as_deref(), Some("niri is busy"));
    assert_eq!(session.retry_at, 2_000 + RETRY_MS);

    // the window to Keep is still the one the trial began with: a failed revert does not reopen it
    assert_eq!(session.deadline, 1_000 + TRIAL_MS);

    // the ticks between the attempt and the deadline: nothing is tried
    let commands = fake.commands.borrow().len();

    for _ in 0..16 {
        fake.at(2_250);
        assert_eq!(control.revert_when_due(&fake, true), None);
    }
    assert_eq!(fake.runs.get(), tried + 1);
    assert_eq!(fake.commands.borrow().len(), commands);

    // and at the deadline it tries again
    fake.at(2_000 + RETRY_MS);
    assert_eq!(
        control.revert_when_due(&fake, true),
        Some(Err(String::from("niri is busy")))
    );
    assert_eq!(fake.runs.get(), tried + 2);

    // an edit niri accepts clears the stale error, so leaving the page reverts at once instead of
    // waiting out the failed revert's deadline
    *fake.refuse.borrow_mut() = None;
    control
        .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
        .unwrap();
    assert_eq!(control.session.as_ref().unwrap().stuck, None);
    assert_eq!(
        control.revert_when_due(&fake, true),
        Some(Ok(Ended::Reverted))
    );
    assert!(control.session.is_none());
}

#[test]
fn an_output_unplugged_during_a_trial_is_nothing_to_put_back() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "HDMI-A-5", Edit::Scale(2.0), None)
        .unwrap();
    fake.outputs
        .borrow_mut()
        .retain(|output| output.name != "HDMI-A-5");

    assert_eq!(control.revert(&fake), Ok(Ended::Reverted));
    assert!(!dir.file().exists());
}

#[test]
fn keep_writes_every_change_of_the_trial_at_once() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(1.5), None)
        .unwrap();
    control
        .try_edit(&fake, "eDP-1", Edit::Scale(1.25), None)
        .unwrap();
    control
        .try_edit(&fake, "eDP-1", Edit::Vrr(Vrr::OnDemand), None)
        .unwrap();

    assert_eq!(
        control.keep(&fake),
        Ok(Ended::Kept(
            String::from("written"),
            Some(String::from("after"))
        ))
    );

    // the last of two changes to the same part, one write
    assert_eq!(
        *fake.written.borrow(),
        [(
            String::from("eDP-1"),
            vec![Edit::Scale(1.25), Edit::Vrr(Vrr::OnDemand)]
        )]
    );

    // the file and session stay until confirm_keep
    assert!(dir.file().exists());
    assert!(control.session.is_some());
    control.confirm_keep();
    assert!(!dir.file().exists());
    assert!(control.session.is_none());
}

#[test]
fn a_keep_the_engine_refuses_leaves_the_trial_to_run_out() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(1.5), None)
        .unwrap();
    fake.cannot_write.set(true);

    assert_eq!(
        control.keep(&fake),
        Err(String::from("niri refuses the config"))
    );
    assert!(control.session.is_some());
    assert!(dir.file().exists());
    assert!(fake.written.borrow().is_empty());

    fake.at(20_000);
    assert_eq!(control.expire(&fake), Some(Ok(Ended::Reverted)));
    assert_eq!(fake.state("eDP-1"), panel());
}

// ---- after a crash ----

fn crashed(dir: &Dir, fake: &Fake) -> Output {
    let mut control = Controller::new(dir.file());
    let before = fake.state("eDP-1");

    control
        .try_edit(fake, "eDP-1", Edit::Scale(2.0), None)
        .unwrap();
    control
        .try_edit(fake, "eDP-1", Edit::Position(10, 20), None)
        .unwrap();

    // and the process dies: the controller goes with it, the file stays
    drop(control);
    assert!(dir.file().exists());

    before
}

#[test]
fn a_fresh_pending_file_puts_the_output_back_at_start() {
    let (dir, fake, _) = setup();
    let before = crashed(&dir, &fake);

    fake.at(60_000);
    fake.commands.borrow_mut().clear();

    let mut control = Controller::new(dir.file());
    let said = control.recover(&fake).unwrap().unwrap();

    assert!(said.contains("eDP-1"), "{said}");
    assert_eq!(fake.state("eDP-1"), before);
    assert!(!dir.file().exists());

    // only what differed was said: the mode and the turn were as before
    let said: Vec<String> = fake
        .commands
        .borrow()
        .iter()
        .map(|(_, args)| args[0].clone())
        .collect();

    assert_eq!(said, ["scale", "position"]);
}

#[test]
fn a_pending_file_older_than_five_minutes_is_deleted_and_not_applied() {
    let (dir, fake, _) = setup();

    crashed(&dir, &fake);
    // An external change modified the output to scale 3.0 (neither applied 2.0 nor previous 1.0)
    let mut modified = fake.state("eDP-1");
    modified.logical.as_mut().unwrap().scale = 3.0;
    fake.outputs.borrow_mut()[0] = modified;

    fake.commands.borrow_mut().clear();
    fake.at(RECOVERY_MS + 2);

    let mut control = Controller::new(dir.file());
    let said = control.recover(&fake).unwrap().unwrap();

    assert!(said.contains("older than 5 minutes"), "{said}");
    assert!(fake.commands.borrow().is_empty());
    assert!(!dir.file().exists());
    assert_eq!(fake.state("eDP-1").logical.unwrap().scale, 3.0);
}

#[test]
fn a_pending_file_that_cannot_be_read_is_deleted() {
    let (dir, fake, _) = setup();

    fs::write(dir.file(), b"{\"output\": \"eDP-1\", \"previous\": {").unwrap();

    let mut control = Controller::new(dir.file());

    assert!(control.recover(&fake).unwrap().is_some());
    assert!(!dir.file().exists());
    assert!(fake.commands.borrow().is_empty());
}

#[test]
fn a_pending_file_waits_for_a_niri_that_does_not_answer() {
    let (dir, fake, _) = setup();

    crashed(&dir, &fake);
    fake.cannot_ask.set(true);

    let mut control = Controller::new(dir.file());

    assert!(control.recover(&fake).is_err());
    assert!(dir.file().exists());

    // and without a file there is nothing to do
    fake.cannot_ask.set(false);
    control.recover(&fake).unwrap();
    assert!(!dir.file().exists());
    assert_eq!(control.recover(&fake), Ok(None));
}

#[test]
fn a_negative_position_is_tried_and_put_back_behind_two_dashes() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "HDMI-A-5", Edit::Position(-1280, -10), None)
        .unwrap();
    assert_eq!(
        fake.state("HDMI-A-5").logical.map(|l| (l.x, l.y)),
        Some((-1280, -10))
    );

    // and back from a negative one
    control
        .try_edit(&fake, "HDMI-A-5", Edit::Scale(2.0), None)
        .unwrap();
    control.revert(&fake).unwrap();
    assert_eq!(fake.state("HDMI-A-5"), dell());
    assert!(!dir.file().exists());

    let mut left = dell();

    left.logical.as_mut().unwrap().x = -1920;
    fake.outputs.borrow_mut()[1] = left;
    control
        .try_edit(&fake, "HDMI-A-5", Edit::Position(0, 0), None)
        .unwrap();
    control.revert(&fake).unwrap();
    assert_eq!(
        fake.state("HDMI-A-5").logical.map(|l| (l.x, l.y)),
        Some((-1920, 0))
    );
    assert!(
        fake.commands
            .borrow()
            .iter()
            .any(|(_, args)| args == &["position", "set", "--", "-1920", "0"])
    );
}

#[test]
fn a_mode_cannot_be_tried_on_an_output_in_a_custom_mode() {
    let (dir, fake, mut control) = setup();
    let mut custom = panel();
    let target = custom.modes[1];

    custom.current = None;
    fake.outputs.borrow_mut()[0] = custom.clone();

    let why = control
        .try_edit(&fake, "eDP-1", Edit::Mode(target), None)
        .unwrap_err();

    assert!(why.contains("custom mode"), "{why}");
    assert!(fake.commands.borrow().is_empty());
    assert!(!dir.file().exists());
    assert!(control.session.is_none());

    // the rest of its settings can be tried: they are put back as they were
    control
        .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
        .unwrap();
    control.revert(&fake).unwrap();
    assert_eq!(fake.state("eDP-1"), custom);
}

// a second screen turned on for a trial, and the first unplugged since
fn only_the_tried_screen_is_left() -> (Dir, Fake, Controller) {
    let (dir, fake, mut control) = setup();

    fake.outputs.borrow_mut()[1].logical = None;
    control
        .try_edit(&fake, "HDMI-A-5", Edit::Enabled(true), None)
        .unwrap();
    assert!(fake.state("HDMI-A-5").enabled());
    fake.outputs
        .borrow_mut()
        .retain(|output| output.name != "eDP-1");
    fake.commands.borrow_mut().clear();

    (dir, fake, control)
}

#[test]
fn a_revert_never_turns_off_the_only_screen_that_is_on() {
    let (dir, fake, mut control) = only_the_tried_screen_is_left();

    let ended = control.revert(&fake).unwrap();

    assert!(matches!(ended, Ended::LeftOn(_)), "{ended:?}");
    assert!(fake.commands.borrow().is_empty());
    assert!(fake.state("HDMI-A-5").enabled());
    assert!(!dir.file().exists());
    assert!(control.session.is_none());
}

#[test]
fn a_recovery_never_turns_off_the_only_screen_that_is_on() {
    let (dir, fake, control) = only_the_tried_screen_is_left();

    // the shell dies there; the next one finds the file
    drop(control);
    assert!(dir.file().exists());

    let said = Controller::new(dir.file()).recover(&fake).unwrap().unwrap();

    assert!(said.contains("left on"), "{said}");
    assert!(fake.commands.borrow().is_empty());
    assert!(fake.state("HDMI-A-5").enabled());
    assert!(!dir.file().exists());
}

#[test]
fn a_screen_turned_on_for_a_trial_is_turned_off_when_another_is_on() {
    let (_dir, fake, mut control) = setup();

    fake.outputs.borrow_mut()[1].logical = None;
    control
        .try_edit(&fake, "HDMI-A-5", Edit::Enabled(true), None)
        .unwrap();

    assert_eq!(control.revert(&fake), Ok(Ended::Reverted));
    assert!(!fake.state("HDMI-A-5").enabled());
}

#[test]
fn a_list_with_no_output_in_it_is_not_every_output_gone() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
        .unwrap();
    fake.outputs.borrow_mut().clear();
    fake.commands.borrow_mut().clear();
    fake.at(2_000);

    // niri not ready: the revert is tried, fails against nothing, and the file waits
    *fake.refuse.borrow_mut() = Some(String::from("no such output"));
    assert!(control.revert(&fake).is_err());
    assert!(dir.file().exists());
    assert!(control.session.is_some());

    // a trial that turned an output on is not ended either: it is not known another is on
    let (dir, fake, mut control) = setup();

    fake.outputs.borrow_mut()[1].logical = None;
    control
        .try_edit(&fake, "HDMI-A-5", Edit::Enabled(true), None)
        .unwrap();
    fake.cannot_ask.set(true);
    assert!(control.revert(&fake).is_err());
    assert!(dir.file().exists());
    assert!(control.session.is_some());

    // the list comes back with the output in it
    fake.cannot_ask.set(false);
    assert_eq!(control.revert(&fake), Ok(Ended::Reverted));
    assert!(!dir.file().exists());
}

#[test]
fn a_recovery_with_no_output_listed_keeps_the_file() {
    let (dir, fake, _) = setup();

    crashed(&dir, &fake);
    fake.outputs.borrow_mut().clear();

    let mut control = Controller::new(dir.file());

    assert!(control.recover(&fake).is_err());
    assert!(dir.file().exists());
}

#[test]
fn a_pending_file_for_an_output_that_is_gone_is_just_deleted() {
    let (dir, fake, _) = setup();

    crashed(&dir, &fake);
    fake.outputs
        .borrow_mut()
        .retain(|output| output.name != "eDP-1");
    fake.commands.borrow_mut().clear();

    let mut control = Controller::new(dir.file());

    control.recover(&fake).unwrap();
    assert!(fake.commands.borrow().is_empty());
    assert!(!dir.file().exists());
}

// ---- what the config and niri say ----

#[test]
fn a_scale_niri_rounds_is_said_once_it_is_kept() {
    let mut shown = panel();

    shown.logical.as_mut().unwrap().scale = 1.3333;

    let said = disagreements(&[&Edit::Scale(1.33), &Edit::Position(0, 0)], &shown);

    assert_eq!(said.len(), 1);
    assert!(
        said[0].contains("scale 1.33") && said[0].contains("1.3333"),
        "{said:?}"
    );

    // what niri shows as written is not mentioned, nor is a vrr on demand
    assert!(disagreements(&[&Edit::Scale(1.3333), &Edit::Vrr(Vrr::OnDemand)], &shown).is_empty());
}

#[test]
fn on_demand_is_known_only_from_the_config() {
    assert_eq!(Vrr::of(false, Some(Vrr::OnDemand)), Vrr::OnDemand);
    assert_eq!(Vrr::of(true, Some(Vrr::OnDemand)), Vrr::OnDemand);
    assert_eq!(Vrr::of(true, None), Vrr::On);
    assert_eq!(Vrr::of(false, Some(Vrr::On)), Vrr::Off);
}

// ---- the text inputs ----

#[test]
fn text_being_typed_is_not_taken_by_a_countdown_or_a_poll() {
    use super::work::should_set;

    // first sight: the input takes what niri shows
    assert!(should_set("0", None, None, false));

    // nothing changed, or the user typed over it and has not applied: left alone
    assert!(!should_set("0", Some("0"), Some("0"), false));
    assert!(!should_set("0", Some("0"), Some("-19"), false));

    // niri changed under text typed: still theirs, until they apply it or give it up
    assert!(!should_set("5", Some("0"), Some("-19"), false));

    // niri changed under text not touched: it follows
    assert!(should_set("5", Some("0"), Some("0"), false));

    // a change of their own just ended: the input shows what niri has, applied or refused
    assert!(should_set("0", Some("0"), Some("-19"), true));
    assert!(should_set("1.25", Some("1.3333"), Some("1.3333"), true));
    assert!(!should_set("0", Some("0"), Some("0"), true));
}

#[test]
fn a_countdown_does_not_ask_for_the_inputs_to_be_set_again() {
    let mut state = work::State::default();
    let trial = |seconds| work::Trial {
        id: 1,
        output: String::from("eDP-1"),
        edits: BTreeMap::new(),
        seconds,
        stuck: None,
    };

    state.set_trial(Some(trial(15)));
    state.set_trial(Some(trial(14)));
    state.set_trial(None);
    assert_eq!(state.generation, 0);

    // a change of theirs that ended does
    state.resync("eDP-1", &['x', 'y']);
    assert_eq!(state.generation, 1);
    assert_eq!(state.resync.len(), 2);
}

// ---- Finding A: keep confirmation ----

#[test]
fn a_keep_is_confirmed_only_after_confirm_keep_is_called() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(1.5), None)
        .unwrap();
    control.keep(&fake).unwrap();

    // the file is still there after keep, before confirm
    assert!(dir.file().exists());
    assert!(control.session.is_some());

    control.confirm_keep();

    // gone after confirm
    assert!(!dir.file().exists());
    assert!(control.session.is_none());
}

#[test]
fn abandon_keep_clears_session_but_leaves_the_file() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(1.5), None)
        .unwrap();
    control.keep(&fake).unwrap();

    control.abandon_keep();

    // the file stays for crash recovery; the session is cleared
    assert!(dir.file().exists());
    assert!(control.session.is_none());
}

#[test]
fn a_saved_change_left_by_a_crash_is_not_put_back_at_start() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
        .unwrap();
    let session = control.session.as_ref().unwrap();
    let (id, requested_at) = (session.id, session.deadline);

    assert_eq!(
        control.keep_request(&fake, id, requested_at),
        Ok(Ended::Kept(
            String::from("written"),
            Some(String::from("after"))
        ))
    );
    // the reload is not confirmed (a timeout, or a SIGKILL during the wait)
    control.abandon_keep();
    drop(control);
    assert!(dir.file().exists());

    fake.commands.borrow_mut().clear();
    let mut fresh = Controller::new(dir.file());
    let said = fresh.recover(&fake).unwrap().unwrap();

    // the config was written, so the record is cleared and niri is not asked to put it back
    assert!(said.contains("saved"), "{said}");
    assert!(!dir.file().exists());
    assert!(fake.commands.borrow().is_empty());
    assert_eq!(fake.state("eDP-1").logical.unwrap().scale, 2.0);
}

#[test]
fn a_pending_file_without_saved_still_puts_the_output_back_at_start() {
    let (dir, fake, _) = setup();
    let before = fake.state("eDP-1");

    // a file an older Kanade wrote, with no `saved` key
    let previous = Previous::of(&before, None);
    let json = serde_json::json!({
        "output": "eDP-1",
        "taken_ms": fake.unix_ms(),
        "previous": previous.to_json(),
        "applied": [Edit::Scale(2.0).to_json()],
    });
    fs::write(dir.file(), json.to_string()).unwrap();

    // niri still shows the unconfirmed trial change
    let mut shown = fake.state("eDP-1");
    shown.logical.as_mut().unwrap().scale = 2.0;
    fake.outputs.borrow_mut()[0] = shown;

    fake.commands.borrow_mut().clear();
    let mut control = Controller::new(dir.file());
    let said = control.recover(&fake).unwrap().unwrap();

    assert!(said.contains("eDP-1"), "{said}");
    assert!(!dir.file().exists());
    assert_eq!(fake.state("eDP-1"), before);
    assert!(
        fake.commands
            .borrow()
            .iter()
            .any(|(_, args)| args[0] == "scale")
    );
}

// ---- Finding B: RunError ----

#[test]
fn a_command_that_does_not_answer_does_not_end_a_trial() {
    let (dir, fake, mut control) = setup();

    fake.hangs.set(true);

    let why = control
        .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
        .unwrap_err();

    assert!(why.contains("timed out"), "{why}");

    // the trial stays: the pending file is there, the session holds the edit, and the
    // deadline can revert it
    assert!(dir.file().exists());
    assert!(control.session.is_some());
    let session = control.session.as_ref().unwrap();
    assert!(session.edits.contains_key(&Field::Scale));

    // the deadline reverts it
    fake.hangs.set(false);
    fake.at(20_000);
    assert_eq!(control.expire(&fake), Some(Ok(Ended::Reverted)));
    assert!(!dir.file().exists());
}

#[test]
fn a_revert_that_does_not_answer_keeps_its_file_and_tries_again() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
        .unwrap();
    fake.hangs.set(true);
    fake.at(2_000);

    // the revert fails with Unanswered, which is still an error to the controller
    let result = control.revert(&fake);
    assert!(result.is_err());
    assert!(dir.file().exists());
    assert!(control.session.is_some());

    let session = control.session.as_ref().unwrap();
    assert!(session.stuck.is_some());
    assert_eq!(session.retry_at, 2_000 + RETRY_MS);

    // when the command works again, the retry succeeds
    fake.hangs.set(false);
    fake.at(2_000 + RETRY_MS);
    assert_eq!(control.expire(&fake), Some(Ok(Ended::Reverted)));
    assert_eq!(fake.state("eDP-1"), panel());
    assert!(!dir.file().exists());
}

#[test]
fn a_keep_whose_load_fails_can_be_reverted() {
    let (dir, fake, mut control) = setup();
    let before = fake.state("eDP-1");

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
        .unwrap();
    control.keep(&fake).unwrap();

    // the load failed: revert puts the output back, the file goes with it
    assert!(dir.file().exists());
    assert_eq!(control.revert(&fake), Ok(Ended::Reverted));
    assert_eq!(fake.state("eDP-1"), before);
    assert!(!dir.file().exists());
}

// ---- run_error ----

// Every way `child::run_typed` can end but a clean exit means niri may have applied the change, so
// the trial and its file stay. A reader that died and output over the cap used to read as niri's
// own refusal, which ends the trial and deletes the file over a change niri may be showing.
#[test]
fn run_error_classifies_child_failures_as_unanswered_and_niri_refusals_as_refused() {
    use super::work::run_error;
    use crate::appearance::child::Error;

    // Kanade's own side of the run failing: niri may have applied the change
    for error in [
        Error::TimedOut(String::from("niri msg output timed out after 2000 ms")),
        Error::Spawn(String::from(
            "could not run niri msg output: No such file or directory (os error 2)",
        )),
        Error::Reader(String::from("could not read niri msg output output")),
        Error::Reader(String::from(
            "could not wait for niri msg output: interrupted",
        )),
        Error::Reader(String::from(
            "niri msg output stdout reader stopped unexpectedly",
        )),
        Error::TooLong(String::from(
            "niri msg output output exceeded 1048576 bytes",
        )),
        Error::Aborted(String::from("appearance sync worker stopped")),
    ] {
        assert!(
            matches!(run_error(error.clone()), RunError::Unanswered(_)),
            "{error:?}"
        );
    }

    // niri's own refusals: it told us no
    for why in [
        "Unknown output \"HDMI-A-5\"",
        "error: unexpected argument '--' found",
    ] {
        assert!(matches!(
            run_error(Error::Exited(String::from(why))),
            RunError::Refused(_)
        ));
    }
}

// ---- Recovery findings fixes ----

#[test]
fn a_recovery_puts_back_every_variable_refresh_rate_mode() {
    for (previous_vrr, applied_vrr) in [
        (Vrr::Off, Vrr::On),
        (Vrr::Off, Vrr::OnDemand),
        (Vrr::On, Vrr::Off),
        (Vrr::On, Vrr::OnDemand),
        (Vrr::OnDemand, Vrr::Off),
        (Vrr::OnDemand, Vrr::On),
    ] {
        let (dir, fake, _) = setup();

        // niri reports on-demand as off (and on only while a window wants it), so a monitor on
        // any of the three modes shows vrr_enabled false just after a trial
        let mut shown = panel();
        shown.vrr_enabled = false;
        fake.outputs.borrow_mut()[0] = shown.clone();

        let previous = Previous {
            enabled: true,
            mode: shown.current_mode().map(|mode| mode.text()),
            scale: 1.0,
            transform: Transform::Normal,
            x: 0,
            y: 0,
            vrr: previous_vrr,
        };
        let json = serde_json::json!({
            "output": "eDP-1",
            "taken_ms": fake.unix_ms(),
            "previous": previous.to_json(),
            "applied": [Edit::Vrr(applied_vrr).to_json()],
        });
        fs::write(dir.file(), json.to_string()).unwrap();

        let mut control = Controller::new(dir.file());
        control.recover(&fake).unwrap();

        let vrr: Vec<Vec<String>> = fake
            .commands
            .borrow()
            .iter()
            .filter(|(_, args)| args[0] == "vrr")
            .map(|(_, args)| args.clone())
            .collect();
        let wanted: Vec<String> = match previous_vrr {
            Vrr::Off => vec!["vrr".into(), "off".into()],
            Vrr::On => vec!["vrr".into(), "on".into()],
            Vrr::OnDemand => vec!["vrr".into(), "--on-demand".into(), "on".into()],
        };

        assert_eq!(vrr, vec![wanted], "{previous_vrr:?} -> {applied_vrr:?}");
        assert!(!dir.file().exists());
    }
}

#[test]
fn advancing_clock_past_deadline_rejects_keep_and_reverts_trial() {
    let (dir, fake, mut control) = setup();
    let before = fake.state("eDP-1");

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(1.5), None)
        .unwrap();
    assert!(control.session.is_some());
    let session = control.session.as_ref().unwrap();
    let id = session.id;
    let deadline = session.deadline;

    // Advance clock past deadline and grace window
    fake.at(deadline + KEEP_GRACE_MS + 100);

    // Keep request with an expired time is rejected and reverts the trial immediately
    let err = control.keep_request(&fake, id, deadline + 1).unwrap_err();
    assert_eq!(err, "The trial has expired");

    // Session is gone, pending file is deleted, and output restored to before
    assert!(control.session.is_none());
    assert!(!dir.file().exists());
    assert_eq!(fake.state("eDP-1"), before);
}

#[test]
fn keep_request_within_grace_window_is_accepted() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(1.5), None)
        .unwrap();
    let session = control.session.as_ref().unwrap();
    let id = session.id;
    let deadline = session.deadline;

    // Requested before deadline, but processed slightly after deadline within KEEP_GRACE_MS
    fake.at(deadline + KEEP_GRACE_MS / 2);

    let res = control.keep_request(&fake, id, deadline);
    assert_eq!(
        res,
        Ok(Ended::Kept(
            String::from("written"),
            Some(String::from("after"))
        ))
    );
    assert!(dir.file().exists());
    assert!(control.session.is_some());
}

#[test]
fn pending_file_older_than_five_minutes_restores_if_trial_state_is_live() {
    let (dir, fake, _) = setup();
    let before = crashed(&dir, &fake);

    // Live state still matches what was applied in trial (scale 2.0, position 10, 20)
    assert_eq!(fake.state("eDP-1").logical.unwrap().scale, 2.0);
    assert_eq!(fake.state("eDP-1").logical.unwrap().x, 10);
    assert_eq!(fake.state("eDP-1").logical.unwrap().y, 20);

    fake.commands.borrow_mut().clear();
    fake.at(RECOVERY_MS + 10_000);

    let mut control = Controller::new(dir.file());
    let said = control.recover(&fake).unwrap().unwrap();

    // Since trial state was still live on niri, recover restored it to previous!
    assert!(said.contains("eDP-1"), "{said}");
    assert_eq!(fake.state("eDP-1"), before);
    assert!(!dir.file().exists());
}

#[test]
fn pending_file_older_than_five_minutes_cleans_up_if_already_matches_previous() {
    let (dir, fake, _) = setup();
    let before = crashed(&dir, &fake);

    // Live state already returned to previous (e.g. external reload)
    fake.outputs.borrow_mut()[0] = before.clone();

    fake.commands.borrow_mut().clear();
    fake.at(RECOVERY_MS + 10_000);

    let mut control = Controller::new(dir.file());
    let said = control.recover(&fake).unwrap().unwrap();

    assert!(said.contains("already as it was"), "{said}");
    assert!(fake.commands.borrow().is_empty());
    assert!(!dir.file().exists());
    assert_eq!(fake.state("eDP-1"), before);
}

#[test]
fn failed_config_reload_undoes_config_before_reverting_output() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(1.5), None)
        .unwrap();
    control.keep(&fake).unwrap();
    let loaded = Some(crate::sources::niri::ConfigLoadState {
        count: 1,
        last_failed: true,
    });

    control.resolve_keep_reload(&fake, loaded);

    assert!(control.session.is_none());
    assert!(!dir.file().exists());
    assert_eq!(fake.state("eDP-1"), panel());
}

#[test]
fn failed_config_undo_keeps_pending_recovery_file_and_reverts_output() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(1.5), None)
        .unwrap();
    control.keep(&fake).unwrap();
    fake.cannot_undo.set(true);
    let loaded = Some(crate::sources::niri::ConfigLoadState {
        count: 1,
        last_failed: true,
    });

    control.resolve_keep_reload(&fake, loaded);

    assert!(control.session.is_none());
    assert!(dir.file().exists());
    assert_eq!(fake.state("eDP-1"), panel());
}

#[test]
fn reload_confirmed_deletes_recovery_file_even_if_niri_differs() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(1.5), None)
        .unwrap();
    control.keep(&fake).unwrap();

    // The pending recovery file and session exist after keep
    assert!(dir.file().exists());
    assert!(control.session.is_some());

    // Niri reloaded successfully (last_failed: false), but disagreed is true (e.g. scale rounded or coords differ)
    let loaded = Some(crate::sources::niri::ConfigLoadState {
        count: 1,
        last_failed: false,
    });
    control.resolve_keep_reload(&fake, loaded);

    // Pending file MUST be deleted and session ended because reload succeeded!
    assert!(!dir.file().exists());
    assert!(control.session.is_none());
}

#[test]
fn a_keep_with_no_reload_seen_is_not_reverted_at_start() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(1.5), None)
        .unwrap();
    control.keep(&fake).unwrap();

    // no ConfigLoaded update was seen in the window: the trial ends and the file records a
    // change already written
    control.resolve_keep_reload(&fake, None);
    assert!(control.session.is_none());
    assert!(dir.file().exists());

    fake.commands.borrow_mut().clear();
    let mut fresh = Controller::new(dir.file());
    let said = fresh.recover(&fake).unwrap().unwrap();

    assert!(said.contains("saved"), "{said}");
    assert!(!dir.file().exists());
    assert!(fake.commands.borrow().is_empty());
    assert_eq!(fake.state("eDP-1").logical.unwrap().scale, 1.5);
}

// the controller is one for the whole process, as it is in the shell: the tests that drive it take
// this, so one test's trial is never the next one's
static CONTROL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn injected_worker_panic_triggers_emergency_revert() {
    let _held = CONTROL_LOCK.lock().unwrap_or_else(|held| held.into_inner());
    let (dir, fake, _) = setup();
    let before = fake.state("eDP-1");

    work::controller(|control| {
        *control = Controller::new(dir.file());
        control
            .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
            .unwrap();
    });

    assert!(work::controller(|c| c.session.is_some()));
    assert!(dir.file().exists());

    let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        panic!("simulated worker thread panic");
    }));
    let payload = panic_result.unwrap_err();

    work::handle_worker_panic(&fake, &*payload);

    // Emergency revert put the output back and deleted the pending file
    assert!(work::controller(|c| c.session.is_none()));
    assert!(!dir.file().exists());
    assert_eq!(fake.state("eDP-1"), before);

    let statuses = crate::supervise::status();
    assert!(statuses.iter().any(|s| s.contains("kanade-outputs")));
}

// A Keep writes the config at once and niri's reload confirms it up to 2 s later. Kanade exiting
// inside that window (a restart, a logout, a `scripts/dev` save) must not put the monitor back: the
// config holds what the user kept, so a revert there would leave the screen as it was before the
// Keep with no pending file and nothing said until niri's next reload.
#[test]
fn an_exit_after_a_keep_and_before_its_reload_does_not_put_the_output_back() {
    let _held = CONTROL_LOCK.lock().unwrap_or_else(|held| held.into_inner());
    let (dir, fake, _) = setup();

    work::controller(|control| {
        *control = Controller::new(dir.file());
        control
            .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
            .unwrap();
        control.keep(&fake).unwrap();

        // the config holds it; niri's reload has not come yet
        assert!(control.session.as_ref().unwrap().kept);
    });

    // the worker dies inside that window
    let panic = std::panic::catch_unwind(|| panic!("simulated worker thread panic"));
    let payload = panic.unwrap_err();
    work::handle_worker_panic(&fake, &*payload);

    // and the exit path, as a restart runs it
    work::revert_on_exit();

    // neither put the output back: the monitor shows what was kept, and the file is a record of a
    // change already written, which the next start clears without touching niri
    assert_eq!(fake.state("eDP-1").logical.unwrap().scale, 2.0);
    assert!(dir.file().exists());

    // and a start after it clears the file without touching niri
    fake.commands.borrow_mut().clear();
    let mut fresh = Controller::new(dir.file());
    let said = fresh.recover(&fake).unwrap().unwrap();

    assert!(said.contains("saved"), "{said}");
    assert!(fake.commands.borrow().is_empty());
    assert_eq!(fake.state("eDP-1").logical.unwrap().scale, 2.0);
    assert!(!dir.file().exists());
}

#[test]
fn keep_request_succeeds_and_prevents_revert_when_writing_pending_file_fails() {
    let (dir, fake, mut control) = setup();

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
        .unwrap();
    let session = control.session.as_ref().unwrap();
    let (id, requested_at) = (session.id, session.deadline);

    struct Restore(PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
            }
        }
    }
    let _guard = Restore(dir.0.clone());

    // Make rewriting the pending file fail (e.g. read-only permissions on file and parent directory)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir.file(), fs::Permissions::from_mode(0o400));
        let _ = fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o500));
    }

    let kept = control.keep_request(&fake, id, requested_at);

    // Config write succeeded, so keep_request succeeds even though writing the pending file failed
    assert_eq!(
        kept,
        Ok(Ended::Kept(
            String::from("written"),
            Some(String::from("after"))
        ))
    );

    // The pending file is deleted or cleared
    let pending_is_cleared = !dir.file().exists()
        || fs::read_to_string(dir.file())
            .map(|text| text.is_empty())
            .unwrap_or(false);
    assert!(pending_is_cleared);

    // Restore permissions so cleanup and recovery can access the directory
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o700));
    }

    // A later start (crash recovery) does not revert niri's output
    fake.commands.borrow_mut().clear();
    let mut fresh = Controller::new(dir.file());
    let _ = fresh.recover(&fake);

    assert!(fake.commands.borrow().is_empty());
    assert_eq!(fake.state("eDP-1").logical.unwrap().scale, 2.0);
}

// ---- Vera's four, each at the seam it crossed ----

// Vera 1: a worker that panicked left no timer. The trial's own clock is the timer, and it is due
// from `retry_at` rather than from anything a tick counted, so it still fires.
#[test]
fn a_trial_whose_worker_died_is_still_due_at_its_deadline() {
    let _held = CONTROL_LOCK.lock().unwrap_or_else(|held| held.into_inner());
    let (dir, fake, _) = setup();
    let before = fake.state("eDP-1");

    // the trial is the controller's, not a local: it survives the worker that began it
    work::controller(|control| {
        *control = Controller::new(dir.file());
        control
            .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
            .unwrap();
    });

    let deadline = work::controller(|control| control.session.as_ref().unwrap().deadline);

    // the worker is dead: nothing ticks, so only the clock's own wait can find the trial due
    fake.at(deadline - 1);
    assert_eq!(
        work::controller(|control| control.wait(fake.now())),
        Some(1),
        "the clock waits on the deadline, not on a tick"
    );

    // a panic in the worker's own thread, taken the way `spawn` takes it
    let panic = std::panic::catch_unwind(|| panic!("simulated worker thread panic"));
    assert!(panic.is_err());

    // nothing has run yet; the clock is what ends it, through the same controller
    assert!(work::controller(|control| control.session.is_some()));
    fake.at(deadline);
    work::clock_due(&fake);

    assert!(work::controller(|control| control.session.is_none()));
    assert!(!dir.file().exists());
    assert_eq!(fake.state("eDP-1"), before);
}

// the clock's wait is `retry_at` and nothing else: a failed revert's retry, and the end of a trial
// the config holds, are the two things that move it
#[test]
fn the_clock_waits_on_the_retry_at_and_never_on_a_kept_trial() {
    let (dir, fake, mut control) = setup();

    assert_eq!(control.wait(fake.now()), None);

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
        .unwrap();
    let session = control.session.clone().unwrap();

    assert_eq!(
        control.wait(fake.now()),
        Some(session.retry_at - fake.now())
    );

    // a failed revert moves the retry, and the clock with it
    *fake.refuse.borrow_mut() = Some(String::from("niri is busy"));
    fake.at(2_000);
    assert!(control.revert(&fake).is_err());

    let session = control.session.clone().unwrap();
    assert_eq!(
        control.wait(fake.now()),
        Some(session.retry_at - fake.now())
    );

    // a Keep the config took is not the clock's to revert: niri's reload decides
    *fake.refuse.borrow_mut() = None;
    control.keep(&fake).unwrap();
    assert!(control.session.as_ref().unwrap().kept);
    assert_eq!(control.wait(fake.now()), None);

    fake.at(session.retry_at + RETRY_MS);

    // the clock looked, found nothing to time, and put nothing back
    assert_eq!(control.revert_when_due(&fake, false), None);
    assert!(control.session.as_ref().unwrap().kept);
    assert!(dir.file().exists());
    assert_eq!(fake.state("eDP-1").logical.unwrap().scale, 2.0);
}

// Vera 2: a recovery could overwrite, or be overwritten by, a new trial's file. Neither side may
// touch the other's: the file is one trial's.
#[test]
fn a_recovery_and_a_new_trial_never_share_the_pending_file() {
    let (dir, fake, _) = setup();
    crashed(&dir, &fake);

    // a fresh controller does not write over a file another Kanade left: that file is the only
    // record that puts that monitor back, so the change is refused rather than the file dropped
    let mut other = Controller::new(dir.file());
    let refused = other
        .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
        .unwrap_err();

    assert!(refused.contains("another Kanade"), "{refused}");
    assert!(other.session.is_none());
    assert!(dir.file().exists());

    // and a controller that holds a trial leaves the file for it: recovery does not take it
    let mut held = Controller::new(dir.file());
    let text = fs::read_to_string(dir.file()).unwrap();
    fs::remove_file(dir.file()).unwrap();
    held.try_edit(&fake, "eDP-1", Edit::Scale(1.25), None)
        .unwrap();
    fs::write(dir.file(), text).unwrap();

    let said = held.recover(&fake).unwrap().unwrap();
    assert!(said.contains("being changed now"), "{said}");
    assert!(held.session.is_some());
    assert!(dir.file().exists());
    assert_eq!(fake.state("eDP-1").logical.unwrap().scale, 1.25);
}

// Vera 3: a Keep pressed after the deadline, but before a tick noticed, still wrote the config.
// The request carries when it was made, so the deadline is checked rather than the tick.
#[test]
fn a_keep_made_after_the_deadline_is_refused_even_before_a_tick() {
    let (dir, fake, mut control) = setup();
    let before = fake.state("eDP-1");

    control
        .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
        .unwrap();
    let session = control.session.clone().unwrap();

    // the press came one millisecond after the window closed, and no tick has run since
    let late = session.deadline + 1;
    fake.at(late);

    let refused = control.keep_request(&fake, session.id, late).unwrap_err();

    assert_eq!(refused, "The trial has expired");
    assert!(control.session.is_none());
    assert!(!dir.file().exists());
    assert_eq!(fake.state("eDP-1"), before);

    // nothing was written to the config either
    assert!(fake.written.borrow().is_empty());
}

// Vera 4: on-demand is invisible in niri's report, so a recovery sent it back on every trial, even
// one that never touched the variable refresh rate. Only a trial that changed it sends it back.
#[test]
fn a_recovery_sends_on_demand_back_only_for_a_trial_that_changed_it() {
    let applied: [(Edit, bool); 2] = [(Edit::Scale(2.0), false), (Edit::Vrr(Vrr::On), true)];

    for (edit, changed_vrr) in applied {
        let (dir, fake, _) = setup();

        let mut shown = panel();
        shown.vrr_enabled = false;
        fake.outputs.borrow_mut()[0] = shown.clone();

        let previous = Previous {
            enabled: true,
            mode: shown.current_mode().map(|mode| mode.text()),
            scale: 1.0,
            transform: Transform::Normal,
            x: 0,
            y: 0,
            vrr: Vrr::OnDemand,
        };
        let json = serde_json::json!({
            "output": "eDP-1",
            "taken_ms": fake.unix_ms(),
            "previous": previous.to_json(),
            "applied": [edit.to_json()],
        });
        fs::write(dir.file(), json.to_string()).unwrap();

        let mut control = Controller::new(dir.file());
        control.recover(&fake).unwrap();

        let sent_on_demand = fake
            .commands
            .borrow()
            .iter()
            .any(|(_, args)| args.iter().any(|arg| arg == "--on-demand"));

        assert_eq!(sent_on_demand, changed_vrr, "{edit:?}");
        assert!(!dir.file().exists());
    }
}

// the clock thread itself, with no worker at all: it sleeps on the trial's own wait, does not fire
// before the deadline, and ends the trial when it comes. This is the case the defect was, since
// nothing else is running to tick.
#[test]
fn the_clock_thread_ends_a_trial_the_worker_never_ticks() {
    use std::time::{Duration, Instant};

    let _held = CONTROL_LOCK.lock().unwrap_or_else(|held| held.into_inner());
    let (dir, fake, _) = setup();
    let before = fake.state("eDP-1");

    // the clock sleeps on real time, so the fake's clock is made to follow it
    fake.follow_real_time();
    let started = Instant::now();

    work::controller(|control| {
        *control = Controller::new(dir.file());
        control
            .try_edit(&fake, "eDP-1", Edit::Scale(2.0), None)
            .unwrap();

        // a deadline a moment away rather than fifteen seconds
        let now = fake.now();
        let session = control.session.as_mut().unwrap();
        session.deadline = now + 400;
        session.retry_at = now + 400;
    });

    let (woken, woke) = std::sync::mpsc::channel();

    // an observer beside the clock, standing in for the worker's own wake: it looks before the
    // deadline, waits for the clock to end the trial, then drops the sender so the clock returns
    let early = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let watched = early.clone();
    let observer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        watched.store(
            work::controller(|control| control.session.is_some()),
            Ordering::Relaxed,
        );

        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until && work::controller(|control| control.session.is_some()) {
            std::thread::sleep(Duration::from_millis(5));
        }

        drop(woken);
    });

    // the clock runs on its own, with no worker and no tick beside it
    work::clock(&woke, &fake);
    observer.join().unwrap();

    assert!(
        early.load(Ordering::Relaxed),
        "the clock ended the trial before its deadline"
    );
    assert!(
        work::controller(|control| control.session.is_none()),
        "the clock never ended the trial"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the clock waited for the trial, not for a tick"
    );
    assert!(!dir.file().exists());
    assert_eq!(fake.state("eDP-1"), before);
}
