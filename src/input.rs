//! Mapeo de teclas configurable (nombres SDL) a acciones.

use crate::config::KeysCfg;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Advance,
    Fullscreen,
    Quit,
    Skip,
    MenuUp,
    MenuDown,
}

pub struct Keymap(HashMap<String, Action>);

impl Keymap {
    pub fn new(k: &KeysCfg) -> Self {
        let mut m = HashMap::new();
        for (list, a) in [
            (&k.advance, Action::Advance),
            (&k.fullscreen, Action::Fullscreen),
            (&k.quit, Action::Quit),
            (&k.skip, Action::Skip),
            (&k.menu_up, Action::MenuUp),
            (&k.menu_down, Action::MenuDown),
        ] {
            for name in list {
                m.insert(name.to_lowercase(), a);
            }
        }
        Self(m)
    }
    pub fn action(&self, key_name: &str) -> Option<Action> {
        self.0.get(&key_name.to_lowercase()).copied()
    }
}
