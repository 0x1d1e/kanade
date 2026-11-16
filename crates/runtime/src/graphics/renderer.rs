mod command;
mod effect;
mod shape;
mod text;

use super::{Area, Corners, Transform};

pub use command::Command;
pub use effect::VALUE_ROWS;

/*
 * collects what widgets draw as commands,
 * so no part of the runtime outside the gpu backend knows how drawing is done
 */
pub struct Renderer {
    commands: Vec<Command>,
    transform: Transform,
}

impl Renderer {
    pub fn new(scale: f32) -> Self {
        // widgets measure in logical pixels, the buffer in real ones
        let horizontal_scale = scale;
        let vertical_scale = scale;

        let transform = Transform::from_scale(horizontal_scale, vertical_scale);

        Self {
            commands: Vec::new(),
            transform,
        }
    }

    pub fn finish(self) -> Vec<Command> {
        self.commands
    }

    pub fn group(&self) -> Self {
        Self {
            commands: Vec::new(),
            transform: self.transform,
        }
    }

    pub fn blend(&mut self, group: Renderer, opacity: f32) {
        // a transparent or empty group draws nothing, so it gets no command, not even a group one
        if opacity <= 0.0 || group.commands.is_empty() {
            return;
        }

        let cuts = group
            .commands
            .iter()
            .any(|command| matches!(command, Command::Cut { .. }));

        // a plain group draws the same inline, and only a separate group costs the gpu extra
        if opacity == 1.0 && !cuts {
            self.commands.extend(group.commands);

            return;
        }

        self.commands.push(Command::Group {
            commands: group.commands,
            opacity,
        });
    }

    // what the group drew only shows inside the rounded rectangle
    pub fn clip(&mut self, group: Renderer, area: Area, radius: impl Into<Corners>) {
        // an empty rectangle shows none of the group
        if area.width <= 0.0 || area.height <= 0.0 {
            return;
        }

        self.commands.push(Command::Clip {
            area,
            radius: radius.into(),
            transform: self.transform,
            commands: group.commands,
        });
    }

    // everything drawn inside goes through the local transform before the renderer's own
    pub fn transformed(&mut self, local: Transform, draw: impl FnOnce(&mut Renderer)) {
        let outer = self.transform;

        self.transform = local.post_concat(outer);

        draw(self);

        self.transform = outer;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::Color;

    fn drawn_group(renderer: &Renderer) -> Renderer {
        let mut group = renderer.group();

        group.rectangle(
            Area::new(0.0, 0.0, 10.0, 10.0),
            Color::rgb(255, 255, 255),
            0.0,
        );

        group
    }

    #[test]
    fn transparent_and_empty_groups_add_no_command() {
        let mut renderer = Renderer::new(1.0);
        let transparent = drawn_group(&renderer);
        let empty = renderer.group();

        renderer.blend(transparent, 0.0);
        renderer.blend(empty, 1.0);
        renderer.blend(renderer.group(), 0.5);

        assert!(renderer.finish().is_empty());
    }

    #[test]
    fn opaque_group_adds_its_commands_inline() {
        let mut renderer = Renderer::new(1.0);
        let group = drawn_group(&renderer);

        renderer.blend(group, 1.0);

        assert_eq!(renderer.finish().len(), 1);
    }
}
