use std::cell::RefCell;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process;

use super::output_value;
use super::*;

// a directory of its own, with a config in it
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str, config: &str) -> Dir {
        let dir = env::temp_dir().join(format!("kanade-niri-config-{name}-{}", process::id()));

        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("niri")).unwrap();
        fs::write(dir.join("niri/config.kdl"), config).unwrap();

        Dir(dir)
    }

    fn config(&self) -> PathBuf {
        self.0.join("niri/config.kdl")
    }

    fn text(&self) -> String {
        fs::read_to_string(self.config()).unwrap()
    }

    fn state(&self) -> PathBuf {
        self.0.join("state/kanade/niri-config")
    }

    // the files beside the config that are not the config
    fn strays(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.0.join("niri"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "config.kdl")
            .collect();

        names.sort();
        names
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// what stands in for niri: the file must be KDL v1 and must not hold `reject-me`; every file it
// was asked about is noted
fn fake(seen: &RefCell<Vec<PathBuf>>) -> impl Fn(&Path) -> Result<(), String> + '_ {
    move |file| {
        seen.borrow_mut().push(file.to_owned());

        let text = fs::read_to_string(file).map_err(|error| error.to_string())?;

        edit::parse(&text)?;

        if text.contains("reject-me") {
            return Err(String::from("reject-me is not a thing (config.kdl:1:1)"));
        }

        Ok(())
    }
}

fn editor<'a>(dir: &Dir, validate: Validate<'a>) -> Editor<'a> {
    Editor {
        file: dir.config(),
        state: dir.state(),
        validate,
        now_ms: || 1_700_000_000_000,
    }
}

const CONFIG: &str = "// mine\r\nlayout {\r\n    gaps 4 // tight\r\n    /-struts { left 8; }\r\n}\r\n\r\nprefer-no-csd\r\n// the end";

#[test]
fn a_single_value_replaces_a_node_with_multiple_values() {
    let dir = Dir::new(
        "replace-multiple-values",
        "layout {\n    focus-ring {\n        active-color 255 0 0 255\n    }\n}\n",
    );
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);

    editor
        .set("layout.focus-ring.active-color", "#00ff00")
        .unwrap();

    assert_eq!(
        dir.text(),
        "layout {\n    focus-ring {\n        active-color \"#00ff00\"\n    }\n}\n"
    );
    assert_eq!(
        editor.get("layout.focus-ring.active-color").unwrap(),
        "\"#00ff00\""
    );
}

#[test]
fn a_change_is_validated_beside_the_file_backed_up_and_only_its_bytes_differ() {
    let dir = Dir::new("change", CONFIG);
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);

    fs::set_permissions(dir.config(), fs::Permissions::from_mode(0o640)).unwrap();

    let said = editor(&dir, &validate).set("layout.gaps", "12").unwrap();

    assert!(said.starts_with("layout.gaps = 12 (was 4)"), "{said}");
    assert_eq!(dir.text(), CONFIG.replace("gaps 4", "gaps 12"));
    assert_eq!(
        fs::metadata(dir.config()).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert!(
        dir.strays().is_empty(),
        "no staged file left: {:?}",
        dir.strays()
    );

    // the original first, then the candidate staged in the same directory
    let seen = seen.borrow();

    assert_eq!(seen[0], dir.config());
    assert_eq!(seen[1].parent(), dir.config().parent());
    assert_ne!(seen[1], dir.config());

    // the old bytes are number 1 and the pinned original
    let ring = Ring::new(&dir.state(), &dir.config());

    assert_eq!(ring.read("1").unwrap().1, CONFIG.as_bytes());
    assert_eq!(ring.read("orig").unwrap().1, CONFIG.as_bytes());

    let bits = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;

    assert_eq!(bits(&dir.state()), 0o700);
    assert_eq!(bits(&ring.orig()), 0o600);
}

#[test]
fn a_config_niri_rejects_is_not_edited() {
    let dir = Dir::new("original", "layout {\n    gaps 4\n");
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let before = dir.text();

    let why = editor(&dir, &validate).set("layout.gaps", "1").unwrap_err();

    assert!(why.contains("as it is"), "{why}");
    assert_eq!(dir.text(), before);
    assert!(dir.strays().is_empty());
    assert!(Ring::new(&dir.state(), &dir.config()).list().is_empty());
    assert_eq!(
        seen.borrow().len(),
        1,
        "the candidate was never even staged"
    );
}

#[test]
fn a_change_niri_rejects_leaves_the_file_and_no_part() {
    let dir = Dir::new("candidate", "layout {\n    gaps 4\n}\n");
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);

    let why = editor(&dir, &validate)
        .set("layout.gaps", "reject-me")
        .unwrap_err();

    // niri names the staged file; the message names the config
    assert!(
        why.contains("rejects the change") && why.contains("reject-me"),
        "{why}"
    );
    assert_eq!(dir.text(), "layout {\n    gaps 4\n}\n");
    assert!(dir.strays().is_empty(), "{:?}", dir.strays());
    assert!(Ring::new(&dir.state(), &dir.config()).list().is_empty());
}

#[test]
fn a_save_by_another_program_during_the_change_is_not_overwritten() {
    let dir = Dir::new("race", "layout {\n    gaps 4\n}\n");
    let config = dir.config();
    let editing = RefCell::new(false);

    // the user's editor saves while niri validates the candidate
    let validate = |file: &Path| -> Result<(), String> {
        if file != config && !*editing.borrow() {
            *editing.borrow_mut() = true;
            fs::write(&config, "layout {\n    gaps 4\n}\n// edited meanwhile\n").unwrap();
        }

        Ok(())
    };

    let why = editor(&dir, &validate).set("layout.gaps", "9").unwrap_err();

    assert!(why.contains("changed on disk"), "{why}");
    assert_eq!(dir.text(), "layout {\n    gaps 4\n}\n// edited meanwhile\n");
    assert!(dir.strays().is_empty());
    assert!(
        Ring::new(&dir.state(), &config).list().is_empty(),
        "the backup of a change that did not happen is dropped"
    );
}

#[test]
fn concurrent_editors_serialize_and_undo_the_committed_changes_in_order() {
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    let dir = Arc::new(Dir::new("concurrent", "layout {\n    gaps 4\n}\n"));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (second_attempt_tx, second_attempt_rx) = mpsc::channel();

    let first_dir = dir.clone();
    let first = std::thread::spawn(move || {
        let validate = |file: &Path| {
            if file != first_dir.config() {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }
            Ok(())
        };
        let editor = Editor {
            file: first_dir.config(),
            state: first_dir.state(),
            validate: &validate,
            now_ms: || 1_700_000_000_000,
        };
        editor.set("layout.gaps", "8")
    });

    // Start an independent editor while the first holds the lock in candidate validation.
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let second_dir = dir.clone();
    let (second_validated_tx, second_validated_rx) = mpsc::channel();
    let second = std::thread::spawn(move || {
        second_attempt_tx.send(()).unwrap();
        let validate = |_: &Path| {
            second_validated_tx.send(()).unwrap();
            Ok(())
        };
        let editor = Editor {
            file: second_dir.config(),
            state: second_dir.state(),
            validate: &validate,
            now_ms: || 1_700_000_000_001,
        };
        editor.set("layout.focus-ring.width", "3")
    });

    second_attempt_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    assert!(
        second_validated_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    );
    release_tx.send(()).unwrap();
    assert!(first.join().unwrap().is_ok());
    assert!(second.join().unwrap().is_ok());
    assert!(dir.text().contains("gaps 8"));
    assert!(dir.text().contains("width 3"));

    let validate = |_: &Path| Ok(());
    let editor = Editor {
        file: dir.config(),
        state: dir.state(),
        validate: &validate,
        now_ms,
    };
    editor.undo().unwrap();
    assert!(dir.text().contains("gaps 8"));
    assert!(!dir.text().contains("width 3"));
}

#[test]
fn undo_works_only_while_the_file_is_as_the_change_left_it() {
    let dir = Dir::new("undo", CONFIG);
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);

    assert!(editor.undo().unwrap_err().contains("nothing to undo"));

    editor.set("layout.gaps", "12").unwrap();
    editor.undo().unwrap();

    assert_eq!(dir.text(), CONFIG);

    // undone again is done again: the undo kept what it replaced
    editor.undo().unwrap();

    assert_eq!(dir.text(), CONFIG.replace("gaps 4", "gaps 12"));

    // an edit by hand since: undo would lose it
    editor.set("layout.gaps", "3").unwrap();

    let by_hand = format!("{}\n// by hand", dir.text());

    fs::write(dir.config(), &by_hand).unwrap();

    let why = editor.undo().unwrap_err();

    assert!(why.contains("changed since"), "{why}");
    assert_eq!(dir.text(), by_hand);
}

#[test]
fn undo_only_refuses_a_change_made_after_the_pages_own() {
    let dir = Dir::new("undo-only", CONFIG);
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);

    // the page's change, with its hash
    let page = editor(&dir, &validate);

    page.set("layout.gaps", "12").unwrap();

    let ours = page.last_change_hash().unwrap();

    assert_eq!(
        ours,
        Ring::new(&dir.state(), &dir.config()).last().unwrap().after
    );

    // then a change from the command line, over the same state, which becomes the ring's newest
    let cli = editor(&dir, &validate);

    cli.set("layout.gaps", "20").unwrap();

    let after_cli = dir.text();

    assert_ne!(cli.last_change_hash().unwrap(), ours);

    // the page's undo would put back what the CLI changed, so it refuses and the file stands
    let why = page.undo_only(Some(&ours)).unwrap_err();

    assert!(why.contains("not this page's change"), "{why}");
    assert_eq!(dir.text(), after_cli);

    // while the CLI's own undo, with no hash to hold it, still undoes the CLI's change
    cli.undo().unwrap();

    assert_eq!(dir.text(), CONFIG.replace("gaps 4", "gaps 12"));
}

#[test]
fn a_bare_node_says_it_is_on_and_not_nothing() {
    let dir = Dir::new("bare", CONFIG);
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);
    let id = OutputId::named("DP-1");

    // on, a bare node: the message says what the file holds, not an empty value
    let said = editor
        .set_output(
            &id,
            "variable-refresh-rate",
            output_value("variable-refresh-rate", "").unwrap(),
        )
        .unwrap();

    assert!(!said.contains("=  ("), "{said}");
    assert!(said.contains("= true (was unset)"), "{said}");

    // set again, nothing changes, and the message still has no gap
    let said = editor
        .set_output(
            &id,
            "variable-refresh-rate",
            output_value("variable-refresh-rate", "").unwrap(),
        )
        .unwrap();

    assert!(!said.contains("  "), "{said}");
    assert!(said.contains("is true already"), "{said}");

    // and off, removed: what was there is the bare node
    let said = editor.remove_output(&id, "variable-refresh-rate").unwrap();

    assert!(said.contains("(was true)"), "{said}");
}

#[test]
fn a_change_niri_reads_only_at_a_start_says_so() {
    let dir = Dir::new("restart", CONFIG);
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);

    let said = editor
        .set("environment.QT_QPA_PLATFORMTHEME", "qt6ct")
        .unwrap();

    assert!(said.contains("when it starts"), "{said}");
    assert!(said.contains("restart"), "{said}");

    // a path niri reads as it goes says nothing of a restart
    let said = editor.set("layout.gaps", "12").unwrap();

    assert!(!said.contains("restart"), "{said}");
}

#[test]
fn backups_list_and_restore_through_the_same_checks() {
    let dir = Dir::new("restore", "layout {\n    gaps 1\n}\n");
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);

    for gaps in ["2", "3", "4"] {
        editor.set("layout.gaps", gaps).unwrap();
    }

    let listed = editor.backups();

    assert_eq!(listed.lines().count(), 4, "{listed}");
    assert!(listed.lines().last().unwrap().starts_with("orig"));

    // 1 is the newest: the file before the last change
    editor.restore("1").unwrap();
    assert!(dir.text().contains("gaps 3"));

    editor.restore("orig").unwrap();
    assert!(dir.text().contains("gaps 1"));

    assert!(editor.restore("99").is_err());
    assert!(editor.restore("x").is_err());

    // a backup that niri would reject now is not put back
    fs::write(dir.config(), "layout {\n    gaps 5\n}\n").unwrap();
    editor.set("layout.gaps", "6").unwrap();

    let ring = Ring::new(&dir.state(), &dir.config());
    let poisoned = ring.list().len();

    fs::write(dir.state().join(&ring.list()[0].file), "reject-me\n").unwrap();

    assert!(
        editor
            .restore("1")
            .unwrap_err()
            .contains("rejects the change")
    );
    assert!(dir.text().contains("gaps 6"));
    assert_eq!(ring.list().len(), poisoned);
}

#[test]
fn two_configs_of_one_name_keep_separate_rings_and_never_restore_each_other() {
    let one = Dir::new("ring-one", "layout {\n    gaps 1\n}\n");
    let two = Dir::new("ring-two", "layout {\n    gaps 100\n}\n");
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    // one state directory for both
    let on = |dir: &Dir| Editor {
        file: dir.config(),
        state: one.state(),
        validate: &validate,
        now_ms: || 1_700_000_000_000,
    };
    let (first, second) = (on(&one), on(&two));

    first.set("layout.gaps", "2").unwrap();
    second.set("layout.gaps", "200").unwrap();
    first.set("layout.gaps", "3").unwrap();

    assert_eq!(
        first.backups().lines().count(),
        3,
        "two backups and its own orig: {}",
        first.backups()
    );
    assert_eq!(second.backups().lines().count(), 2, "{}", second.backups());

    // orig and the numbers are each config's own
    second.restore("orig").unwrap();
    assert!(two.text().contains("gaps 100"));
    assert!(
        one.text().contains("gaps 3"),
        "the other config is untouched"
    );

    first.restore("orig").unwrap();
    assert!(one.text().contains("gaps 1"));
    assert!(two.text().contains("gaps 100"));

    first.restore("2").unwrap();
    assert!(one.text().contains("gaps 2"));
    assert!(two.text().contains("gaps 100"));

    // `.last` and a backup of the other are refused
    second.set("layout.gaps", "300").unwrap();

    let other = Ring::new(&one.state(), &two.config()).list()[0]
        .file
        .clone();

    assert!(
        Ring::new(&one.state(), &one.config())
            .named(&other)
            .is_err()
    );

    // each has its own last change: undoing one's leaves the other
    first.undo().unwrap();
    assert!(one.text().contains("gaps 1"));
    assert!(two.text().contains("gaps 300"));
}

#[test]
fn a_node_an_include_sets_is_refused_and_named() {
    let dir = Dir::new(
        "include",
        "include \"mine.kdl\"\nlayout {\n    gaps 4\n}\ninclude optional=true \"kanade.kdl\"\n",
    );

    fs::write(
        dir.0.join("niri/mine.kdl"),
        "layout {\n    gaps 9\n    border { width 2; }\n}\n",
    )
    .unwrap();
    fs::write(
        dir.0.join("niri/kanade.kdl"),
        "layout {\n    focus-ring {\n        active-color \"#fff\"\n    }\n}\n",
    )
    .unwrap();

    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);

    // the config's own gaps comes after the include, so it wins and is the setting
    assert_eq!(editor.get("layout.gaps").unwrap(), "4");

    // but an include sets it too, so it is not the config's to edit, and a reader must see that
    assert_eq!(
        editor.read("layout.gaps").unwrap(),
        Reading::Overridden("4".into(), dir.0.join("niri/mine.kdl"))
    );
    assert!(editor.set("layout.gaps", "5").is_err());
    assert_eq!(
        editor.read("layout.border.width").unwrap(),
        Reading::Included("2".into(), dir.0.join("niri/mine.kdl"))
    );

    // an include's node, and the sync's, are not the config's to edit
    let why = editor.set("layout.border.width", "5").unwrap_err();

    assert!(
        why.contains("mine.kdl") && why.contains("an include"),
        "{why}"
    );

    let why = editor
        .set("layout.focus-ring.active-color", "#000")
        .unwrap_err();

    assert!(
        why.contains("kanade.kdl") && why.contains("appearance.sync"),
        "{why}"
    );
    assert!(editor.remove("layout.border.width").is_err());
    assert_eq!(
        editor.get("layout.border.width").unwrap(),
        format!("2  # from {}", dir.0.join("niri/mine.kdl").display())
    );

    // but a node the includes leave alone is
    editor.set("layout.focus-ring.width", "3").unwrap();
    assert!(dir.text().contains("focus-ring {\n        width 3"));
}

#[test]
fn an_include_loop_or_a_missing_include_is_a_refusal() {
    let dir = Dir::new("loop", "include \"config.kdl\"\n");
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);

    let why = editor(&dir, &validate).set("layout.gaps", "1").unwrap_err();

    assert!(why.contains("includes itself"), "{why}");

    fs::write(dir.config(), "include \"gone.kdl\"\n").unwrap();

    let why = editor(&dir, &validate).set("layout.gaps", "1").unwrap_err();

    assert!(why.contains("gone.kdl"), "{why}");
    assert_eq!(dir.text(), "include \"gone.kdl\"\n");
}

#[test]
fn a_symlinked_or_system_config_is_read_only() {
    let dir = Dir::new("symlink", "layout {\n    gaps 4\n}\n");
    let real = dir.0.join("dotfiles.kdl");
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);

    fs::rename(dir.config(), &real).unwrap();
    symlink(&real, dir.config()).unwrap();

    let editor = editor(&dir, &validate);
    let why = editor.set("layout.gaps", "1").unwrap_err();

    assert!(why.contains("symbolic link"), "{why}");
    assert!(editor.undo().is_err());
    assert_eq!(
        fs::read_to_string(&real).unwrap(),
        "layout {\n    gaps 4\n}\n"
    );
    assert!(dir.strays().is_empty());

    // reading is fine
    assert_eq!(editor.get("layout.gaps").unwrap(), "4");

    // a hard link is another name of a file that may be shared: not replaced either
    fs::remove_file(dir.config()).unwrap();
    fs::hard_link(&real, dir.config()).unwrap();

    let why = editor.set("layout.gaps", "1").unwrap_err();

    assert!(why.contains("hard link"), "{why}");
    assert_eq!(
        fs::read_to_string(&real).unwrap(),
        "layout {\n    gaps 4\n}\n"
    );
    assert!(dir.strays().is_empty());

    let system = Editor {
        file: PathBuf::from("/etc/niri/config.kdl"),
        state: dir.state(),
        validate: &validate,
        now_ms,
    };
    let why = system.set("layout.gaps", "1").unwrap_err();

    assert!(
        why.contains("system config") && why.contains("cp /etc/niri/config.kdl"),
        "{why}"
    );
}

#[test]
fn unset_things_read_as_unset_and_flags_as_false() {
    let dir = Dir::new("get", "layout {\n    border {\n        off\n    }\n}\n");
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);

    assert!(editor.get("layout.gaps").unwrap_err().contains("not set"));
    assert_eq!(editor.get("layout.border.off").unwrap(), "true");
    assert_eq!(editor.get("layout.focus-ring.off").unwrap(), "false");
    assert_eq!(editor.get("layout.border").unwrap(), "{ ... }");
    assert!(editor.remove("layout.gaps").unwrap().contains("not set"));
    assert!(Ring::new(&dir.state(), &dir.config()).list().is_empty());
}

// what niri prints, with its color
const REJECTED: &str = "\x1b[31mError:\x1b[0m   × error loading config\n  ├─▶ error parsing\n  ╰─▶ error parsing KDL\n\nError:   × found `x`, expected `.`, `;`, `=`, `E`, `\\`, `e`, `{`, newline, whitespace\n  │ or end of input\n   ╭─[bad.kdl:1:1]\n 1 │ layout {\n 2 │   gaps 12x\n   ·          ┬\n   ·          ╰── unexpected token\n 3 │ }\n   ╰────\n";

#[test]
fn niris_verdict_is_its_exit_code_and_its_first_specific_error() {
    assert_eq!(
        verdict(false, REJECTED).unwrap_err(),
        "found `x`, expected `.`, `;`, `=`, `E`, `\\`, `e`, `{`, newline, whitespace or end of input (bad.kdl:1:1)"
    );

    // a zero exit with an error printed is no pass
    assert!(verdict(true, REJECTED).is_err());
    assert!(verdict(true, "\x1b[2m2026\x1b[0m INFO niri: config is valid\n").is_ok());
    assert!(verdict(false, "").is_err());

    let included = "Error:   × error loading config\n  ├─▶ error parsing\n  ╰─▶ error parsing KDL\n\nError:   × failed to read included config from \"nope.kdl\": No such file or directory\n  │ (os error 2)\n   ╭─[inc.kdl:1:1]\n";

    assert_eq!(
        verdict(false, included).unwrap_err(),
        "failed to read included config from \"nope.kdl\": No such file or directory (os error 2) (inc.kdl:1:1)"
    );

    let unreadable = "Error:   × error loading config\n  ├─▶ error reading \"/nonexistent\"\n  ╰─▶ No such file or directory (os error 2)\n";

    assert_eq!(
        verdict(false, unreadable).unwrap_err(),
        "No such file or directory (os error 2)"
    );
}

fn niri_runs() -> bool {
    Command::new("niri")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

// the one test on the real niri; it has nothing to run on a machine without it
#[test]
fn the_real_niri_validates_a_change_and_refuses_a_bad_one() {
    if !niri_runs() {
        eprintln!("niri is not installed: not run");
        return;
    }

    let dir = Dir::new("real", CONFIG);
    let editor = editor(&dir, &niri_validate);

    assert_eq!(editor.check(), Ok(String::from("valid")));

    editor.set("layout.gaps", "12").unwrap();
    editor.set("layout.focus-ring.width", "2").unwrap();
    editor.set("input.keyboard.xkb.layout", "de").unwrap();
    editor.remove("prefer-no-csd").unwrap();

    // a value niri does not take is niri's refusal, not Kanade's
    let why = editor.set("layout.gaps", "wide").unwrap_err();

    assert!(why.contains("rejects the change"), "{why}");
    assert!(dir.text().contains("gaps 12"));
    assert!(dir.strays().is_empty(), "{:?}", dir.strays());

    editor.undo().unwrap();
    assert!(dir.text().contains("prefer-no-csd"));

    // v2 syntax is not read by niri 26.04, so the config as it is fails
    fs::write(dir.config(), "layout {\n    gaps #true\n}\n").unwrap();
    assert!(
        editor
            .set("layout.gaps", "1")
            .unwrap_err()
            .contains("as it is")
    );
}

#[test]
fn a_batch_is_one_backup_one_undo_and_all_or_nothing() {
    let dir = Dir::new("batch", "layout {\n    gaps 4\n}\n");
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);

    // one op the file refuses (an include-free reject) spoils the batch: nothing is written
    let before = dir.text();
    assert!(
        editor
            .apply(&[
                ("layout.gaps", Some("8")),
                ("layout.focus-ring.width", Some("reject-me"))
            ])
            .is_err()
    );
    assert_eq!(dir.text(), before);
    assert!(dir.strays().is_empty());

    // a batch of two is one change: one backup, and one undo puts both back
    editor
        .apply(&[
            ("layout.gaps", Some("8")),
            ("layout.focus-ring.off", Some("true")),
        ])
        .unwrap();
    assert_eq!(
        dir.text(),
        "layout {\n    gaps 8\n    focus-ring {\n        off\n    }\n}\n"
    );
    assert!(editor.backups().contains("1\t"), "{}", editor.backups());
    assert!(!editor.backups().contains("2\t"), "{}", editor.backups());

    editor.undo().unwrap();
    assert_eq!(dir.text(), before);
}

// ---- output blocks ----

const MONITORS: &str = "\u{feff}// monitors\r\noutput \"dp-1\" {\r\n    scale 1\r\n}\r\n\r\noutput \"Dell Inc. P1917S 0x1234\" {\r\n    scale 2 // hidpi\r\n    position x=0 y=0\r\n}\r\noutput \"DP-1\" {\r\n    scale 3\r\n}\r\n// end";

fn dell() -> OutputId {
    OutputId {
        connector: "DP-2".into(),
        make: Some("Dell Inc.".into()),
        model: Some("P1917S".into()),
        serial: Some("0x1234".into()),
    }
}

#[test]
fn a_monitor_has_the_first_block_that_names_it_by_connector_or_by_make_model_serial() {
    let by_connector = OutputId::named("DP-1");

    assert!(by_connector.matches("dp-1") && by_connector.matches("DP-1"));
    assert!(!by_connector.matches("Dell Inc. P1917S 0x1234"));
    assert!(dell().matches("dell inc. p1917s 0X1234") && dell().matches("dp-2"));
    assert!(!dell().matches("Dell Inc. P1917S 0x1235") && !dell().matches("DP-1"));

    // what the EDID lacks reads `Unknown`, as in niri
    let bare = OutputId {
        connector: "HDMI-A-1".into(),
        make: Some("Acme".into()),
        model: None,
        serial: None,
    };

    assert!(bare.matches("Acme Unknown Unknown") && !bare.matches("Acme"));

    assert_eq!(dell().block_name(), "Dell Inc. P1917S 0x1234");
    assert_eq!(bare.block_name(), "HDMI-A-1");
    assert_eq!(
        OutputId {
            connector: "eDP-1".into(),
            ..dell()
        }
        .block_name(),
        "eDP-1"
    );

    let dir = Dir::new("outputs-first", MONITORS);
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);

    // file order: "dp-1" wins over the later "DP-1", the name's case is no difference
    let said = editor.get_output(&by_connector, "scale").unwrap();

    assert!(said.starts_with("1  # "), "{said}");
    assert!(
        said.contains("shadowed"),
        "the shadowed block is said: {said}"
    );

    editor
        .set_output(&by_connector, "scale", Value::Float(1.5))
        .unwrap();

    assert_eq!(
        dir.text(),
        MONITORS.replacen("scale 1\r\n", "scale 1.5\r\n", 1)
    );

    // the alias reaches the block by the monitor's names, the connector it sits on notwithstanding
    assert_eq!(editor.get_output(&dell(), "position").unwrap(), "x=0 y=0");
    editor.set_output(&dell(), "scale", Value::Int(1)).unwrap();
    assert!(dir.text().contains("scale 1 // hidpi"), "{}", dir.text());
}

#[test]
fn a_monitor_without_a_block_gets_one_at_the_end_and_a_reset_takes_it_away() {
    let dir = Dir::new("outputs-new", MONITORS);
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);
    let id = OutputId::named("HDMI-A-1");

    assert!(
        editor
            .get_output(&id, "scale")
            .unwrap_err()
            .contains("not set")
    );

    editor.set_output(&id, "scale", Value::Int(2)).unwrap();
    editor
        .set_output(&id, "position", Value::parse_props("x=1920 y=0").unwrap())
        .unwrap();

    assert_eq!(
        dir.text(),
        format!(
            "{MONITORS}\r\n\r\noutput \"HDMI-A-1\" {{\r\n    scale 2\r\n    position x=1920 y=0\r\n}}\r\n"
        )
    );

    // one change for the lot, so one undo
    let said = editor.reset_output(&id, OUTPUT_KEYS).unwrap();

    assert!(said.contains("HDMI-A-1"), "{said}");
    // the blank line it was set off by goes with it; the file ended without a newline and now ends with one
    assert_eq!(dir.text(), format!("{MONITORS}\r\n"));

    editor.undo().unwrap();
    assert!(dir.text().contains("position x=1920"));

    // a block with something else in it stays, with what Kanade does not set
    editor.undo().unwrap();
    editor.undo().unwrap();
    fs::write(
        dir.config(),
        "output \"A\" {\n    scale 2\n    focus-at-startup\n}\noutput \"B\" {\n    scale 2\n}\n",
    )
    .unwrap();

    editor
        .reset_output(&OutputId::named("A"), OUTPUT_KEYS)
        .unwrap();
    editor
        .reset_output(&OutputId::named("B"), OUTPUT_KEYS)
        .unwrap();

    assert_eq!(dir.text(), "output \"A\" {\n    focus-at-startup\n}\n");
}

#[test]
fn an_include_that_names_the_monitor_makes_it_the_includes_to_edit_even_through_an_alias() {
    let dir = Dir::new(
        "outputs-include",
        "output \"DP-9\" {\n    scale 1\n}\ninclude \"mine.kdl\"\noutput \"Dell Inc. P1917S 0x1234\" {\n    scale 2\n}\n",
    );

    fs::write(
        dir.0.join("niri/mine.kdl"),
        "output \"dp-2\" {\n    scale 3\n}\n",
    )
    .unwrap();

    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);
    let before = dir.text();

    // niri uses the include's block (it comes first, by connector), and Kanade will not edit it,
    // nor the config's own block, which the include's would not let show
    let why = editor
        .set_output(&dell(), "scale", Value::Int(1))
        .unwrap_err();

    assert!(why.contains("mine.kdl") && why.contains("include"), "{why}");
    assert!(editor.remove_output(&dell(), "scale").is_err());
    assert!(editor.reset_output(&dell(), OUTPUT_KEYS).is_err());
    assert_eq!(dir.text(), before);
    assert_eq!(
        editor.read_output(&dell(), "scale").unwrap().reading,
        Reading::Included("3".into(), dir.0.join("niri/mine.kdl"))
    );

    // a monitor the include does not name is the config's
    editor
        .set_output(&OutputId::named("DP-9"), "scale", Value::Int(2))
        .unwrap();
    assert!(dir.text().contains("scale 2\n}\ninclude"));

    // and the include's own file is untouched
    assert_eq!(
        fs::read_to_string(dir.0.join("niri/mine.kdl")).unwrap(),
        "output \"dp-2\" {\n    scale 3\n}\n"
    );
}

#[test]
fn an_include_output_block_is_blocked_even_when_it_only_has_unmanaged_keys() {
    let dir = Dir::new("outputs-include-unmanaged", "include \"mine.kdl\"\n");
    fs::write(
        dir.0.join("niri/mine.kdl"),
        "output \"DP-1\" {\n    focus-at-startup\n}\n",
    )
    .unwrap();

    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);
    let id = OutputId::named("DP-1");

    // The Outputs page cannot edit the block even though Kanade owns none of its keys.
    assert!(
        editor
            .output_editable(&id)
            .unwrap_err()
            .contains("mine.kdl")
    );
    let read = editor.read_output(&id, "scale").unwrap();
    assert_eq!(read.reading, Reading::Unset);
    assert_eq!(read.block_owner, Some(dir.0.join("niri/mine.kdl")));
}

#[test]
fn an_include_output_block_matched_by_edid_is_not_editable() {
    let dir = Dir::new("outputs-include-edid", "include \"mine.kdl\"\n");
    fs::write(
        dir.0.join("niri/mine.kdl"),
        "output \"AU Optronics 0x7EAD 0x00007EAD\" {\n    focus-at-startup\n}\n",
    )
    .unwrap();

    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);
    let id = OutputId {
        connector: String::from("eDP-1"),
        make: Some(String::from("AU Optronics")),
        model: Some(String::from("0x7EAD")),
        serial: Some(String::from("0x00007EAD")),
    };

    assert!(
        editor
            .output_editable(&id)
            .unwrap_err()
            .contains("mine.kdl")
    );
}

#[test]
fn a_mode_with_custom_or_a_modeline_is_not_kanades_to_touch() {
    let dir = Dir::new(
        "outputs-custom",
        "output \"A\" {\n    mode custom=true \"1920x1080@60.000\"\n}\noutput \"B\" {\n    modeline 173.00 1920 2048 2248 2576 1080 1083 1088 1120 \"-hsync\" \"+vsync\"\n}\noutput \"C\" {\n    mode \"1920x1080@60.000\"\n}\n",
    );
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);
    let before = dir.text();

    for (name, key) in [("A", "mode"), ("B", "modeline")] {
        let id = OutputId::named(name);

        assert!(
            editor
                .set_output(&id, key, Value::Str("1280x720@60.000".into()))
                .is_err(),
            "{name} {key}"
        );
        assert!(editor.remove_output(&id, key).is_err(), "{name} {key}");
    }

    assert!(
        editor
            .reset_output(&OutputId::named("A"), OUTPUT_KEYS)
            .is_err()
    );
    // a modeline is not among what a reset removes, so its block stays as it is
    editor
        .reset_output(&OutputId::named("B"), OUTPUT_KEYS)
        .unwrap();
    assert_eq!(dir.text(), before);

    // a plain mode is kanade's
    editor
        .set_output(
            &OutputId::named("C"),
            "mode",
            Value::Str("1280x720@60.000".into()),
        )
        .unwrap();
    assert!(dir.text().contains("mode \"1280x720@60.000\""));
}

#[test]
fn the_output_form_is_a_name_in_quotes_with_a_dotted_key_after_it() {
    let form = |path: &str| output_form(path).unwrap().unwrap();

    assert_eq!(
        form("output[\"DP-1\"]"),
        (OutputId::named("DP-1"), String::new())
    );
    assert_eq!(
        form("output[\"Dell Inc. X \\\"Y\\\" a.b\\\\c\"].variable-refresh-rate"),
        (
            OutputId::named("Dell Inc. X \"Y\" a.b\\c"),
            String::from("variable-refresh-rate")
        )
    );

    assert!(output_form("output.scale").is_none() && output_form("layout.gaps").is_none());

    for bad in [
        "output[DP-1].scale",
        "output[\"DP-1\"",
        "output[\"DP-1\"]scale",
        "output[\"\"]",
        "output[\"a\\n\"]",
        "output[\"a\"].",
    ] {
        assert!(output_form(bad).unwrap().is_err(), "{bad}");
    }
}

#[test]
fn the_command_line_reaches_a_block_by_its_name_whatever_it_holds() {
    let dir = Dir::new("outputs-cli", MONITORS);
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);

    editor
        .set("output[\"Dell Inc. P1917S 0x1234\"].transform", "90")
        .unwrap();
    editor
        .set(
            "output[\"Dell Inc. P1917S 0x1234\"].variable-refresh-rate",
            "on-demand=true",
        )
        .unwrap();
    editor
        .set("output[\"New \\\"One\\\"\"].off", "true")
        .unwrap();

    assert!(dir.text().contains("transform \"90\""), "{}", dir.text());
    assert!(dir.text().contains("variable-refresh-rate on-demand=true"));
    assert!(
        dir.text()
            .ends_with("output r#\"New \"One\"\"# {\r\n    off\r\n}\r\n"),
        "{}",
        dir.text()
    );
    assert_eq!(
        editor.get("output[\"New \\\"One\\\"\"].off").unwrap(),
        "true"
    );
    assert!(editor.get("output[\"DP-1\"]").unwrap().contains("scale 1"));

    editor.remove("output[\"New \\\"One\\\"\"]").unwrap();
    assert!(!dir.text().contains("New"));

    // the plain dotted path still leaves repeated nodes alone
    assert!(editor.set("output.scale", "2").is_err());
    assert!(editor.set("output[\"DP-1\"]", "2").is_err());
}

// every option of an output block, in the forms the settings will send, through the real niri
#[test]
fn niri_accepts_every_value_of_an_output_block() {
    if !niri_runs() {
        eprintln!("niri is not installed: not run");
        return;
    }

    let dir = Dir::new("outputs-real", "// mine\nprefer-no-csd\n");
    let editor = editor(&dir, &niri_validate);
    let name = "Dell Inc. P1917S 0x1234";
    let id = OutputId::named(name);

    for (key, value) in [
        ("scale", "1.5"),
        ("scale", "2"),
        ("transform", "normal"),
        ("transform", "90"),
        ("transform", "180"),
        ("transform", "270"),
        ("transform", "flipped"),
        ("transform", "flipped-90"),
        ("transform", "flipped-180"),
        ("transform", "flipped-270"),
        ("position", "x=-1920 y=100"),
        ("mode", "\"2560x1440@143.912\""),
        ("mode", "\"1920x1080\""),
        ("variable-refresh-rate", "on-demand=true"),
        ("variable-refresh-rate", ""),
        ("off", "true"),
    ] {
        let value = output_value(key, value).unwrap();

        editor
            .set_output(&id, key, value)
            .unwrap_or_else(|why| panic!("{key}: {why}"));
    }

    editor.remove_output(&id, "variable-refresh-rate").unwrap();
    assert_eq!(editor.check(), Ok(String::from("valid")));

    // and a second block for another monitor, then everything gone again
    editor
        .set_output(&OutputId::named("eDP-1"), "scale", Value::Int(1))
        .unwrap();
    editor.reset_output(&id, OUTPUT_KEYS).unwrap();
    editor
        .reset_output(&OutputId::named("eDP-1"), OUTPUT_KEYS)
        .unwrap();

    assert_eq!(dir.text(), "// mine\nprefer-no-csd\n");
}

#[test]
fn creating_and_resetting_a_block_again_and_again_returns_the_same_bytes() {
    for (name, config) in [
        ("plain", "layout {\n    gaps 4\n}\n"),
        ("nonewline", "layout {\n    gaps 4\n}"),
        ("crlf", "layout {\r\n    gaps 4\r\n}\r\n"),
        (
            "include",
            "layout {\n    gaps 4\n}\n\ninclude optional=true \"kanade.kdl\"\n",
        ),
        (
            "include-crlf",
            "layout {\r\n    gaps 4\r\n}\r\n\r\ninclude optional=true \"kanade.kdl\"\r\n",
        ),
        ("only-include", "include optional=true \"kanade.kdl\"\n"),
    ] {
        let dir = Dir::new(&format!("outputs-cycle-{name}"), config);
        let seen = RefCell::new(Vec::new());
        let validate = fake(&seen);
        let editor = editor(&dir, &validate);
        let id = OutputId::named("HDMI-A-1");

        editor.set_output(&id, "scale", Value::Int(2)).unwrap();

        let made = dir.text();

        editor.reset_output(&id, OUTPUT_KEYS).unwrap();

        let first = dir.text();

        for _ in 0..3 {
            editor.set_output(&id, "scale", Value::Int(2)).unwrap();
            assert_eq!(dir.text(), made, "{name}: a block is made the same way");
            editor.reset_output(&id, OUTPUT_KEYS).unwrap();
            assert_eq!(
                dir.text(),
                first,
                "{name}: reset leaves what it left before"
            );
        }

        // a file that ended with a newline is back as it was; one that did not gains one
        if name != "nonewline" {
            assert_eq!(first, config, "{name}");
        } else {
            assert_eq!(first, format!("{config}\n"));
        }
    }
}

#[test]
fn a_reset_that_would_show_a_shadowed_block_is_refused_and_the_note_is_current() {
    let dir = Dir::new(
        "outputs-shadow",
        "output \"DP-1\" {\n    scale 1\n}\noutput \"dp-1\" {\n    scale 3\n}\n",
    );
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);
    let id = OutputId::named("DP-1");
    let before = dir.text();

    let why = editor.reset_output(&id, OUTPUT_KEYS).unwrap_err();

    assert!(why.contains("duplicate") && why.contains("dp-1"), "{why}");
    assert_eq!(dir.text(), before, "nothing was removed, keys included");

    // an edit says what is true after it
    let said = editor.set_output(&id, "scale", Value::Int(2)).unwrap();

    assert!(said.contains("shadowed"), "{said}");

    // with the duplicate gone there is no note and a reset works
    fs::write(dir.config(), "output \"DP-1\" {\n    scale 1\n}\n").unwrap();

    let said = editor.set_output(&id, "scale", Value::Int(2)).unwrap();

    assert!(!said.contains("shadowed"), "{said}");
    editor.reset_output(&id, OUTPUT_KEYS).unwrap();
    assert_eq!(dir.text(), "");
}

#[test]
fn the_command_line_warns_that_it_cannot_see_a_block_named_by_make_model_and_serial() {
    let dir = Dir::new(
        "outputs-warn",
        "output \"Dell Inc. P1917S 0x1234\" {\n    scale 2\n}\n",
    );
    let seen = RefCell::new(Vec::new());
    let validate = fake(&seen);
    let editor = editor(&dir, &validate);

    let said = editor.set("output[\"DP-2\"].scale", "1").unwrap();

    assert!(
        said.contains("Dell Inc. P1917S 0x1234") && said.contains("shadowed"),
        "{said}"
    );

    // the second change finds the block and has nothing to warn about
    let said = editor.set("output[\"DP-2\"].scale", "1.5").unwrap();

    assert!(!said.contains("make, model"), "{said}");
}
