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
    cannot_ask: Cell<bool>,
    clock: Cell<u64>,
    unix: Cell<u64>,

    // whether the pending file existed at each command
    file: PathBuf,
    file_at_commands: RefCell<Vec<bool>>,
}

impl Fake {
    fn new(outputs: Vec<Output>, file: &Path) -> Fake {
        Fake {
            outputs: RefCell::new(outputs),
            commands: RefCell::default(),
            written: RefCell::default(),
            refuse: RefCell::default(),
            cannot_write: Cell::new(false),
            cannot_ask: Cell::new(false),
            clock: Cell::new(1_000),
            unix: Cell::new(1_700_000_000_000),
            file: file.to_owned(),
            file_at_commands: RefCell::default(),
        }
    }

    fn at(&self, millis: u64) {
        self.clock.set(millis);
        self.unix.set(self.unix.get() + millis);
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

    fn run(&self, name: &str, args: &[String]) -> Result<(), String> {
        self.file_at_commands.borrow_mut().push(self.file.exists());

        // niri's CLI reads a negative number as a flag, unless `--` comes before it
        let dashes = args.iter().position(|arg| arg == "--");

        for (at, arg) in args.iter().enumerate() {
            if arg.starts_with('-')
                && arg.len() > 1
                && arg.parse::<f64>().is_ok()
                && !dashes.is_some_and(|dashes| dashes < at)
            {
                return Err(format!("error: unexpected argument '{arg}' found"));
            }
        }

        if let Some(why) = self.refuse.borrow().clone() {
            return Err(why);
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

    fn write(&self, output: &Output, edits: &[&Edit]) -> Result<String, String> {
        if self.cannot_write.get() {
            return Err(String::from("niri refuses the config"));
        }

        self.written.borrow_mut().push((
            output.name.clone(),
            edits.iter().map(|edit| (*edit).clone()).collect(),
        ));

        Ok(String::from("written"))
    }

    fn now(&self) -> u64 {
        self.clock.get()
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
    assert_eq!(session.deadline, 2_000 + RETRY_MS);

    *fake.refuse.borrow_mut() = None;
    fake.at(7_000);

    assert_eq!(control.expire(&fake), Some(Ok(Ended::Reverted)));
    assert_eq!(fake.state("eDP-1"), panel());
    assert!(!dir.file().exists());
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
        Ok(Ended::Kept(String::from("written")))
    );

    // the last of two changes to the same part, one write
    assert_eq!(
        *fake.written.borrow(),
        [(
            String::from("eDP-1"),
            vec![Edit::Scale(1.25), Edit::Vrr(Vrr::OnDemand)]
        )]
    );
    assert!(!dir.file().exists());
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
    fake.commands.borrow_mut().clear();
    fake.at(RECOVERY_MS + 2);

    let mut control = Controller::new(dir.file());
    let said = control.recover(&fake).unwrap().unwrap();

    assert!(said.contains("older than 5 minutes"), "{said}");
    assert!(fake.commands.borrow().is_empty());
    assert!(!dir.file().exists());
    assert_eq!(fake.state("eDP-1").logical.unwrap().scale, 2.0);
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
