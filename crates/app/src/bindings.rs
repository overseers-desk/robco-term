//! The appliance's own chords: what a key held with modifiers does to the
//! terminal, as shipped and as the config file's `[bindings]` table moves
//! them.
//!
//! The keyboard has two layers here, and this is the upper one, consulted
//! first: a chord bound here is a key the program never sees. The lower
//! one is the keytab, [`crate::input`], which says what bytes an unbound
//! key sends. [`SHIPPED`] is `docs/controls.md` as data; the file's rows
//! overlay it, a row on a shipped trigger replacing it in place, a row on a
//! new trigger joining the end, the later of two rows on one trigger
//! standing. A trigger is an exact set of modifiers on the key the layout
//! produces, so `Alt+Shift+PageUp` is not `Alt+PageUp`, and a letter matches
//! without regard to case because X11 hands the shifted letter up in upper
//! case.

use winit::keyboard::{Key, ModifiersState, NamedKey};

use config::KeyBinding;

/// What a chord does to the terminal. No payload: the digit a channel chord
/// names is read off the key that matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    NewChannel,
    CloseChannel,
    NextChannel,
    PrevChannel,
    MoveChannelLeft,
    MoveChannelRight,
    PageBankUp,
    PageBankDown,
    SelectChannel,
    StoreChannel,
    OpenPicker,
    Copy,
    Paste,
    OpenFind,
    FoldBank,
    NewWindow,
    CloseWindow,
}

/// The one table the parser, [`Action::name`] and `docs/config.md` share.
const ACTIONS: &[(Action, &str)] = &[
    (Action::NewChannel, "new_channel"),
    (Action::CloseChannel, "close_channel"),
    (Action::NextChannel, "next_channel"),
    (Action::PrevChannel, "prev_channel"),
    (Action::MoveChannelLeft, "move_channel_left"),
    (Action::MoveChannelRight, "move_channel_right"),
    (Action::PageBankUp, "page_bank_up"),
    (Action::PageBankDown, "page_bank_down"),
    (Action::SelectChannel, "select_channel"),
    (Action::StoreChannel, "store_channel"),
    (Action::OpenPicker, "open_picker"),
    (Action::Copy, "copy"),
    (Action::Paste, "paste"),
    (Action::OpenFind, "open_find"),
    (Action::FoldBank, "fold_bank"),
    (Action::NewWindow, "new_window"),
    (Action::CloseWindow, "close_window"),
];

impl Action {
    pub fn name(self) -> &'static str {
        ACTIONS
            .iter()
            .find(|(action, _)| *action == self)
            .map(|(_, name)| *name)
            .expect("every action is in the table")
    }

    pub fn from_name(name: &str) -> Option<Action> {
        ACTIONS
            .iter()
            .find(|(_, known)| *known == name)
            .map(|(action, _)| *action)
    }
}

/// What a trigger is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bound {
    Action(Action),
    /// Bytes for the program: `ESC` and then the row's `esc` string.
    Esc(Vec<u8>),
    /// The key continues down the chain as if unbound, to the keytab and so
    /// to the program.
    Pass,
    /// The key is taken and nothing is sent.
    Swallow,
}

/// The key half of a trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySpec {
    /// A character the layout produces, held folded to lower case.
    Char(char),
    Named(NamedKey),
    /// Any of the ten digits, consulted after every exact row.
    Digit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trigger {
    pub key: KeySpec,
    pub mods: ModifiersState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub trigger: Trigger,
    pub bound: Bound,
}

/// The chords as shipped: `docs/controls.md`, and the shell's window keys.
/// `chord` is the platform's chord modifier ([`crate::chord::modifier_mask`]).
const SHIPPED: &[(&str, &str, Action)] = &[
    ("digit", "chord", Action::SelectChannel),
    ("digit", "chord | shift", Action::StoreChannel),
    ("t", "ctrl | shift", Action::NewChannel),
    ("t", "chord | shift", Action::OpenPicker),
    ("w", "ctrl | shift", Action::CloseChannel),
    ("pageup", "ctrl", Action::PrevChannel),
    ("pagedown", "ctrl", Action::NextChannel),
    ("left", "ctrl | shift", Action::MoveChannelLeft),
    ("right", "ctrl | shift", Action::MoveChannelRight),
    ("pageup", "chord", Action::PageBankUp),
    ("pagedown", "chord", Action::PageBankDown),
    ("c", "ctrl | shift", Action::Copy),
    ("v", "ctrl | shift", Action::Paste),
    ("f", "ctrl | shift", Action::OpenFind),
    ("b", "ctrl | shift", Action::FoldBank),
    ("n", "ctrl | shift", Action::NewWindow),
    ("q", "ctrl | shift", Action::CloseWindow),
];

/// The chords in force: the shipped set with the file's rows laid over it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bindings {
    rows: Vec<Binding>,
}

impl Bindings {
    /// The shipped set alone, which is what an absent `[bindings]` table means.
    pub fn shipped() -> Self {
        let rows = SHIPPED
            .iter()
            .map(|(key, with, action)| Binding {
                trigger: Trigger {
                    key: parse_key(key).expect("a shipped key parses"),
                    mods: parse_with(with).expect("shipped modifiers parse"),
                },
                bound: Bound::Action(*action),
            })
            .collect();
        Bindings { rows }
    }

    /// The shipped set with `rows` laid over it. A row that does not parse
    /// is logged with its place and left out, the chord it meant to move
    /// standing at its shipped meaning; the appliance has no way to say so
    /// on the glass, and the other rows of the same save still land.
    pub fn build(rows: &[KeyBinding]) -> Self {
        let mut bindings = Self::shipped();
        for (index, row) in rows.iter().enumerate() {
            match Binding::from_row(row) {
                Ok(binding) => bindings.merge(binding),
                Err(why) => log::warn!(
                    "[bindings] row {index} (key {:?}, with {:?}) left out: {why}",
                    row.key,
                    row.with
                ),
            }
        }
        bindings
    }

    fn merge(&mut self, binding: Binding) {
        match self.rows.iter_mut().find(|row| row.trigger == binding.trigger) {
            Some(slot) => *slot = binding,
            None => self.rows.push(binding),
        }
    }

    /// What `key` held with exactly `mods` is bound to. An exact row wins
    /// over the digit class, so a row on `3` outranks the shipped `digit`.
    pub fn lookup(&self, key: &Key, mods: ModifiersState) -> Option<&Bound> {
        let spec = match key {
            Key::Character(text) => KeySpec::Char(single_char(text)?),
            Key::Named(named) => KeySpec::Named(*named),
            _ => return None,
        };
        let exact = self.rows.iter().find(|row| row.trigger.mods == mods && row.trigger.key == spec);
        if exact.is_some() {
            return exact.map(|row| &row.bound);
        }
        match spec {
            KeySpec::Char(c) if c.is_ascii_digit() => self
                .rows
                .iter()
                .find(|row| row.trigger.mods == mods && row.trigger.key == KeySpec::Digit)
                .map(|row| &row.bound),
            _ => None,
        }
    }
}

impl Binding {
    fn from_row(row: &KeyBinding) -> Result<Binding, String> {
        let key = parse_key(&row.key)?;
        let mods = parse_with(&row.with)?;
        let bound = match (row.action.is_empty(), row.esc.is_empty()) {
            (false, true) => match row.action.as_str() {
                "pass" => Bound::Pass,
                "swallow" => Bound::Swallow,
                name => Bound::Action(
                    Action::from_name(name).ok_or_else(|| format!("unknown action {name:?}"))?,
                ),
            },
            (true, false) => {
                let mut bytes = vec![0x1b];
                bytes.extend_from_slice(row.esc.as_bytes());
                Bound::Esc(bytes)
            }
            (true, true) => return Err("neither action nor esc".to_string()),
            (false, false) => return Err("both action and esc".to_string()),
        };
        Ok(Binding {
            trigger: Trigger { key, mods },
            bound,
        })
    }
}

/// The one character of a key's text, folded, or none for a string of more.
fn single_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(fold(c)),
        _ => None,
    }
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// `ctrl | shift`: the modifiers of a row, any order, any case, empty for
/// none. `chord` is the platform's chord modifier; `cmd` is Super wherever
/// it is typed.
pub fn parse_with(with: &str) -> Result<ModifiersState, String> {
    let mut mods = ModifiersState::empty();
    for token in with.split('|').map(str::trim).filter(|token| !token.is_empty()) {
        mods |= match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ModifiersState::CONTROL,
            "shift" => ModifiersState::SHIFT,
            "alt" | "option" | "opt" => ModifiersState::ALT,
            "super" | "cmd" | "command" => ModifiersState::SUPER,
            "chord" => crate::chord::modifier_mask(),
            other => return Err(format!("unknown modifier {other:?}")),
        };
    }
    Ok(mods)
}

const F_KEYS: [NamedKey; 24] = [
    NamedKey::F1,
    NamedKey::F2,
    NamedKey::F3,
    NamedKey::F4,
    NamedKey::F5,
    NamedKey::F6,
    NamedKey::F7,
    NamedKey::F8,
    NamedKey::F9,
    NamedKey::F10,
    NamedKey::F11,
    NamedKey::F12,
    NamedKey::F13,
    NamedKey::F14,
    NamedKey::F15,
    NamedKey::F16,
    NamedKey::F17,
    NamedKey::F18,
    NamedKey::F19,
    NamedKey::F20,
    NamedKey::F21,
    NamedKey::F22,
    NamedKey::F23,
    NamedKey::F24,
];

/// A single character, or a named key as `docs/config.md` spells them.
pub fn parse_key(key: &str) -> Result<KeySpec, String> {
    if let Some(c) = single_char(key) {
        return Ok(KeySpec::Char(c));
    }
    let named = match key.to_ascii_lowercase().as_str() {
        "digit" => return Ok(KeySpec::Digit),
        "pageup" => NamedKey::PageUp,
        "pagedown" => NamedKey::PageDown,
        "home" => NamedKey::Home,
        "end" => NamedKey::End,
        "insert" => NamedKey::Insert,
        "delete" => NamedKey::Delete,
        "enter" | "return" => NamedKey::Enter,
        "tab" => NamedKey::Tab,
        "escape" | "esc" => NamedKey::Escape,
        "space" => NamedKey::Space,
        "backspace" => NamedKey::Backspace,
        "left" => NamedKey::ArrowLeft,
        "right" => NamedKey::ArrowRight,
        "up" => NamedKey::ArrowUp,
        "down" => NamedKey::ArrowDown,
        f => match f.strip_prefix('f').and_then(|n| n.parse::<usize>().ok()) {
            Some(n) if (1..=F_KEYS.len()).contains(&n) => F_KEYS[n - 1],
            _ => return Err(format!("unknown key {key:?}")),
        },
    };
    Ok(KeySpec::Named(named))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL_SHIFT: ModifiersState = ModifiersState::CONTROL.union(ModifiersState::SHIFT);

    fn row(key: &str, with: &str, action: &str, esc: &str) -> KeyBinding {
        KeyBinding {
            key: key.to_string(),
            with: with.to_string(),
            action: action.to_string(),
            esc: esc.to_string(),
        }
    }

    fn character(text: &str) -> Key {
        Key::Character(text.into())
    }

    #[test]
    fn modifiers_parse_in_any_order_case_and_alias() {
        assert_eq!(parse_with("ctrl | shift").unwrap(), CTRL_SHIFT);
        assert_eq!(parse_with("SHIFT|Control").unwrap(), CTRL_SHIFT);
        assert_eq!(parse_with("option").unwrap(), ModifiersState::ALT);
        assert_eq!(parse_with("cmd").unwrap(), ModifiersState::SUPER);
        assert_eq!(parse_with("").unwrap(), ModifiersState::empty());
        assert!(parse_with("meta").is_err());
    }

    #[test]
    fn chord_is_the_platforms_own_modifier() {
        assert_eq!(parse_with("chord").unwrap(), crate::chord::modifier_mask());
        assert_eq!(
            parse_with("chord | shift").unwrap(),
            crate::chord::modifier_mask() | ModifiersState::SHIFT
        );
    }

    #[test]
    fn keys_parse_to_a_folded_character_a_named_key_or_the_digit_class() {
        assert_eq!(parse_key("B").unwrap(), KeySpec::Char('b'));
        assert_eq!(parse_key("pageup").unwrap(), KeySpec::Named(NamedKey::PageUp));
        assert_eq!(parse_key("F5").unwrap(), KeySpec::Named(NamedKey::F5));
        assert_eq!(parse_key("f24").unwrap(), KeySpec::Named(NamedKey::F24));
        assert_eq!(parse_key("digit").unwrap(), KeySpec::Digit);
        assert!(parse_key("f25").is_err());
        assert!(parse_key("hyper").is_err());
    }

    #[test]
    fn the_shipped_set_answers_the_documented_chords() {
        let shipped = Bindings::shipped();
        assert_eq!(shipped.lookup(&character("B"), CTRL_SHIFT), Some(&Bound::Action(Action::FoldBank)));
        assert_eq!(shipped.lookup(&character("t"), CTRL_SHIFT), Some(&Bound::Action(Action::NewChannel)));
        assert_eq!(
            shipped.lookup(&Key::Named(NamedKey::PageUp), ModifiersState::CONTROL),
            Some(&Bound::Action(Action::PrevChannel))
        );
        assert_eq!(
            shipped.lookup(&character("3"), crate::chord::modifier_mask()),
            Some(&Bound::Action(Action::SelectChannel))
        );
        // Exact modifiers: Shift on the pager chord is another trigger.
        assert_eq!(
            shipped.lookup(&Key::Named(NamedKey::PageUp), crate::chord::modifier_mask() | ModifiersState::SHIFT),
            None
        );
        assert_eq!(shipped.lookup(&character("b"), ModifiersState::CONTROL), None);
    }

    #[test]
    fn a_row_on_a_shipped_trigger_replaces_it_and_a_new_one_joins() {
        let built = Bindings::build(&[
            row("c", "ctrl | shift", "pass", ""),
            row("y", "ctrl | shift", "copy", ""),
            row("y", "ctrl | shift", "swallow", ""),
        ]);
        assert_eq!(built.lookup(&character("C"), CTRL_SHIFT), Some(&Bound::Pass));
        assert_eq!(built.lookup(&character("y"), CTRL_SHIFT), Some(&Bound::Swallow), "the later row stands");
        assert_eq!(built.rows.len(), SHIPPED.len() + 1);
        assert_eq!(built.rows[11].trigger.key, KeySpec::Char('c'), "replaced in place");
    }

    #[test]
    fn an_exact_row_outranks_the_digit_class() {
        let built = Bindings::build(&[row("3", "chord", "open_find", "")]);
        assert_eq!(
            built.lookup(&character("3"), crate::chord::modifier_mask()),
            Some(&Bound::Action(Action::OpenFind))
        );
        assert_eq!(
            built.lookup(&character("4"), crate::chord::modifier_mask()),
            Some(&Bound::Action(Action::SelectChannel))
        );
    }

    #[test]
    fn esc_is_the_escape_byte_and_the_string() {
        let built = Bindings::build(&[row("f5", "", "", "[15~")]);
        assert_eq!(
            built.lookup(&Key::Named(NamedKey::F5), ModifiersState::empty()),
            Some(&Bound::Esc(b"\x1b[15~".to_vec()))
        );
    }

    #[test]
    fn a_row_that_does_not_parse_leaves_the_shipped_chord_standing() {
        let built = Bindings::build(&[
            row("c", "ctrl | shift", "no_such_action", ""),
            row("v", "ctrl | hyper", "pass", ""),
            row("f", "ctrl | shift", "", ""),
            row("w", "ctrl | shift", "pass", "[1~"),
        ]);
        assert_eq!(built, Bindings::shipped());
    }

    #[test]
    fn every_action_names_itself_both_ways() {
        for (action, name) in ACTIONS {
            assert_eq!(action.name(), *name);
            assert_eq!(Action::from_name(name), Some(*action));
        }
    }
}
