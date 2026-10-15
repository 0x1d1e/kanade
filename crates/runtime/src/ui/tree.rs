//! The windows' nodes, read off the targets of their last frame, and the names that find them.

use std::collections::HashMap;

use crate::input::Target;

use super::{Mode, Node, REDACTED, Value};

/*
 * the nodes of one frame's targets, in drawing order, so a parent comes before what is in it. A node
 * is named by the names of its parents and its own, and a sibling of a name already taken is
 * numbered (`item#2`), so a node keeps its id until a sibling of its name before it comes or goes
 */
pub(crate) fn nodes(targets: &[Target], pressed: Option<usize>, mode: Mode) -> Vec<Node> {
    let mut nodes: Vec<Node> = Vec::new();

    // the nodes the target being looked at is inside: where each is in `nodes`, and where it ends
    let mut open: Vec<(usize, usize)> = Vec::new();
    let mut taken: HashMap<(Option<usize>, &str), u32> = HashMap::new();

    for (index, target) in targets.iter().enumerate() {
        while open.last().is_some_and(|&(_, end)| index >= end) {
            open.pop();
        }

        let Some(semantics) = &target.semantics else {
            continue;
        };

        // a target scaled to nothing has no place in the window
        let Some((x, y, width, height)) = target.bounds_in_window() else {
            continue;
        };

        let parent = open.last().map(|&(node, _)| node);

        let count = taken.entry((parent, semantics.name)).or_default();

        *count += 1;

        let own = match *count {
            1 => String::from(semantics.name),
            number => format!("{}#{number}", semantics.name),
        };

        let id = match parent {
            Some(parent) => format!("{}/{own}", nodes[parent].id),
            None => own,
        };

        // content is said only in the mode that allows it, whatever the target kept
        let content = |text: &String| match mode {
            Mode::Content => text.clone(),
            _ => String::from(REDACTED),
        };

        let value = match (semantics.level, &semantics.text) {
            (Some(level), _) => Some(Value::Level(level)),
            (None, Some(text)) => Some(Value::Text(content(text))),
            (None, None) => None,
        };

        open.push((nodes.len(), target.end.max(index + 1)));

        nodes.push(Node {
            id,
            parent,
            role: semantics.role,
            label: semantics
                .label
                .clone()
                .or_else(|| semantics.content.as_ref().map(content)),
            value,
            bounds: [x, y, width, height],
            disabled: semantics.disabled,
            selected: semantics.selected,
            checked: semantics.checked,
            focused: semantics.focused,
            pressed: pressed == Some(index),
            secret: semantics.secret,
            target: index,
        });
    }

    nodes
}

// whether `node` is the one `name` says in the tier: its id, the end of its id, or its label
fn says(node: &Node, tier: usize, name: &str, ending: &str) -> bool {
    match tier {
        0 => node.id == name,
        1 => node.id.ends_with(ending),
        _ => node
            .label
            .as_deref()
            .is_some_and(|label| label != REDACTED && label.eq_ignore_ascii_case(name)),
    }
}

/*
 * the node a name stands for: an id as `describe` gives it, else the end of one (`wifi` for
 * `controls/wifi`), else a label as it is said (`Firefox`), whichever of these finds only one
 * node first. A name that fits several is refused with them listed. A label is matched as said, so
 * a redacted one finds nothing, and a name cannot be used to guess at what is redacted
 */
pub(crate) fn resolve<'a>(nodes: &'a [Node], name: &str) -> Result<&'a Node, String> {
    let ending = format!("/{name}");

    for tier in 0..3 {
        let found: Vec<&Node> = nodes
            .iter()
            .filter(|node| says(node, tier, name, &ending))
            .collect();

        match found[..] {
            [] => {}
            [node] => return Ok(node),
            _ => {
                let ids: Vec<&str> = found.iter().map(|node| node.id.as_str()).collect();

                return Err(format!(
                    "{name} is not one node, say one of {}",
                    ids.join(", ")
                ));
            }
        }
    }

    Err(format!("no node named {name}"))
}

#[cfg(test)]
mod tests {
    use crate::graphics::Area;
    use crate::ui::{Role, Semantics};
    use crate::widgets::Widget;
    use crate::{Column, Rectangle};

    use super::*;

    fn node(name: &'static str, semantics: impl FnOnce(Semantics) -> Semantics) -> Rectangle {
        let semantics = semantics(Semantics::new(Role::Button, name)).scrubbed(crate::ui::mode());

        let mut rectangle = Rectangle::new().width(40.0).height(10.0);

        rectangle.semantics = Some(semantics);

        rectangle.on_click(|_| {})
    }

    // a panel of buttons, the way a view builds one, collected as a window's frame is
    fn frame(items: &[&'static str], mode: Mode) -> Vec<Node> {
        crate::ui::store(mode);

        let buttons: Vec<Box<dyn Widget>> = items
            .iter()
            .map(|&label| Box::new(node("item", |s| s.content(label))) as Box<dyn Widget>)
            .collect();

        let mut panel = Rectangle::new()
            .width(40.0)
            .height(100.0)
            .child(Column::new(buttons));

        panel.semantics = Some(Semantics::new(Role::Region, "panel"));

        let mut targets = Vec::new();

        panel.collect_targets(Area::new(0.0, 0.0, 40.0, 100.0), &mut targets);

        nodes(&targets, None, mode)
    }

    // the mode is global, so the tests that set it do not run beside one another
    static MODE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // content is said only when the mode is Content: not by label, not by a text input's value, and a
    // password's text never reaches a target at all
    #[test]
    fn content_is_redacted_unless_asked_for_and_a_password_never_is() {
        let _mode = MODE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        let say = |mode| {
            frame(&["secret title"], mode)
                .iter()
                .filter_map(|node| node.label.clone())
                .collect::<Vec<_>>()
        };

        assert_eq!(say(Mode::Structure), [REDACTED]);
        assert_eq!(say(Mode::Content), ["secret title"]);

        // a target kept from a mode that said content is redacted as the mode is now
        crate::ui::store(Mode::Content);
        let kept = Semantics::new(Role::Item, "kept").content("title");
        let mut target = Target::new(Area::new(0.0, 0.0, 1.0, 1.0), Default::default());

        target.semantics = Some(kept);

        let said = nodes(&[target], None, Mode::Structure);

        assert_eq!(said[0].label.as_deref(), Some(REDACTED));

        let password = Semantics::new(Role::TextInput, "password")
            .input("password", "hunter2", true, false)
            .scrubbed(Mode::Content);

        assert!(password.text.is_none());
        assert!(password.secret);

        let plain = Semantics::new(Role::TextInput, "search")
            .input("search", "query", false, false)
            .scrubbed(Mode::Structure);

        assert_eq!(plain.text.as_deref(), Some(""));

        crate::ui::store(Mode::Off);
    }

    // siblings of one name are numbered in order, parents lead their ids, and an id stays while the
    // siblings after it come and go
    #[test]
    fn ids_are_paths_and_hold_while_later_siblings_change() {
        let _mode = MODE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        let ids = |items: &[&'static str]| {
            frame(items, Mode::Structure)
                .into_iter()
                .map(|node| (node.id, node.parent))
                .collect::<Vec<_>>()
        };

        let three = ids(&["a", "b", "c"]);

        assert_eq!(
            three,
            [
                (String::from("panel"), None),
                (String::from("panel/item"), Some(0)),
                (String::from("panel/item#2"), Some(0)),
                (String::from("panel/item#3"), Some(0)),
            ]
        );

        assert_eq!(ids(&["a", "b"]), three[..3]);

        crate::ui::store(Mode::Off);
    }

    // a full id finds its node, and so does the end of one; a label finds it only as it is said
    #[test]
    fn a_name_finds_one_node_and_a_redacted_label_finds_none() {
        let _mode = MODE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        let tree = frame(&["a", "b"], Mode::Structure);

        assert_eq!(resolve(&tree, "panel/item#2").unwrap().id, "panel/item#2");
        assert_eq!(resolve(&tree, "panel").unwrap().id, "panel");
        assert_eq!(resolve(&tree, "item#2").unwrap().id, "panel/item#2");
        assert!(resolve(&tree, "nothing").is_err());
        assert!(resolve(&tree, "a").is_err());

        let tree = frame(&["a", "b"], Mode::Content);

        assert_eq!(resolve(&tree, "B").unwrap().id, "panel/item#2");

        crate::ui::store(Mode::Off);
    }
}
