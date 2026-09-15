//! User keybinding overrides.
//!
//! Kando's default keymap is compiled in (see [`super::keymap`]). This module
//! layers a user-supplied TOML file on top of it, so keys can be matched to
//! whatever the rest of a person's terminal setup uses:
//!
//! ```toml
//! # ~/.config/kando/keys.toml
//! [normal]
//! b = "find_next"     # nvim-style search stepping
//! B = "find_prev"
//! S = "start_sort"
//! s = "none"          # unbind
//!
//! [space]
//! q = "quit"
//! ```
//!
//! The file is global, not per-board, so a keymap follows you to every board
//! and every machine the dotfiles land on. Text-entry modes (input, filter,
//! confirm, picker) are deliberately not rebindable: those keys are how you
//! type.

use std::collections::HashMap;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers};

use super::action::Action;
use crate::app::Mode;

/// A mode whose keys can be rebound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindMode {
    Normal,
    Space,
    Goto,
    GotoColumn,
    Column,
    ColMove,
    Filter,
    Template,
    Detail,
    Metrics,
    Help,
}

impl BindMode {
    /// The rebindable mode for a runtime mode, or `None` for text entry.
    pub fn of(mode: &Mode) -> Option<Self> {
        Some(match mode {
            Mode::Normal => Self::Normal,
            Mode::Space => Self::Space,
            Mode::Goto => Self::Goto,
            Mode::GotoColumn => Self::GotoColumn,
            Mode::Column => Self::Column,
            Mode::ColMove => Self::ColMove,
            Mode::FilterMenu => Self::Filter,
            Mode::Template => Self::Template,
            Mode::CardDetail { .. } => Self::Detail,
            Mode::Metrics { .. } => Self::Metrics,
            Mode::Help { .. } => Self::Help,
            Mode::Input { .. } | Mode::Filter { .. } | Mode::Confirm { .. } | Mode::Picker { .. } => {
                return None
            }
        })
    }

    /// A Mode value standing for this bind mode, for looking up default keys.
    pub fn to_mode(self) -> Mode {
        match self {
            Self::Normal => Mode::Normal,
            Self::Space => Mode::Space,
            Self::Goto => Mode::Goto,
            Self::GotoColumn => Mode::GotoColumn,
            Self::Column => Mode::Column,
            Self::ColMove => Mode::ColMove,
            Self::Filter => Mode::FilterMenu,
            Self::Template => Mode::Template,
            Self::Detail => Mode::CardDetail { scroll: 0 },
            Self::Metrics => Mode::Metrics { scroll: 0 },
            Self::Help => Mode::Help {
                scroll: 0,
                page: crate::app::HelpPage::Keybindings,
            },
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "normal" => Self::Normal,
            "space" => Self::Space,
            "goto" => Self::Goto,
            "goto_column" => Self::GotoColumn,
            "column" => Self::Column,
            "col_move" => Self::ColMove,
            "filter" => Self::Filter,
            "template" => Self::Template,
            "detail" => Self::Detail,
            "metrics" => Self::Metrics,
            "help" => Self::Help,
            _ => return None,
        })
    }
}

/// One rebound key.
#[derive(Debug)]
struct Override {
    /// `None` means the key is unbound.
    action: Option<Action>,
    /// The action name as written in the file, for describing new bindings in
    /// the help overlay.
    name: String,
}

/// Keys the user has rebound, on top of the built-in defaults.
#[derive(Debug, Default)]
pub struct KeyConfig {
    overrides: HashMap<(BindMode, KeyCode), Override>,
}

impl KeyConfig {
    /// Where the keymap lives: `$XDG_CONFIG_HOME/kando/keys.toml`, falling back
    /// to `~/.config/kando/keys.toml`.
    pub fn path() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
            return Some(PathBuf::from(dir).join("kando/keys.toml"));
        }
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/kando/keys.toml"))
    }

    /// Load the user's keymap. A missing file is not an error — it just means
    /// the defaults stand. A malformed one returns the defaults plus a message
    /// to show the user, because a typo in a config file should never stop a
    /// board from opening.
    pub fn load() -> (Self, Option<String>) {
        let Some(path) = Self::path() else {
            return (Self::default(), None);
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), None)
            }
            Err(e) => return (Self::default(), Some(format!("keys.toml: {e}"))),
        };

        match Self::parse(&text) {
            Ok(config) => (config, None),
            Err(e) => (Self::default(), Some(format!("keys.toml: {e}"))),
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let raw: HashMap<String, HashMap<String, String>> =
            toml::from_str(text).map_err(|e| e.message().to_string())?;

        let mut overrides = HashMap::new();
        for (mode_name, bindings) in raw {
            let mode = BindMode::from_name(&mode_name)
                .ok_or_else(|| format!("unknown mode '{mode_name}'"))?;
            for (key_name, action_name) in bindings {
                let key = parse_key(&key_name)
                    .ok_or_else(|| format!("[{mode_name}]: unknown key '{key_name}'"))?;
                let action = parse_action(&action_name).ok_or_else(|| {
                    format!("[{mode_name}] {key_name}: unknown action '{action_name}'")
                })?;
                overrides.insert((mode, key), Override { action, name: action_name });
            }
        }

        Ok(Self { overrides })
    }

    /// The user's binding for a key, if they set one. The outer `Option` is
    /// "did the user say anything", the inner one is "is it bound".
    pub fn lookup(&self, mode: BindMode, key: KeyCode, modifiers: KeyModifiers) -> Option<&Option<Action>> {
        // Ctrl-C and friends keep their built-in meaning: an override on `c`
        // must not swallow the quit binding.
        if !modifiers.difference(KeyModifiers::SHIFT).is_empty() {
            return None;
        }
        self.overrides.get(&(mode, key)).map(|o| &o.action)
    }

    /// Which key the user bound to `action`, for showing real keys in help.
    pub fn key_for(&self, mode: BindMode, action: &Action) -> Option<KeyCode> {
        self.overrides
            .iter()
            .find(|((m, _), o)| *m == mode && o.action.as_ref() == Some(action))
            .map(|((_, key), _)| *key)
    }

    /// Every key the user bound in `mode`, as (key, action, written name).
    /// Sorted, so the help overlay doesn't reshuffle between frames.
    pub fn bindings_for(&self, mode: BindMode) -> Vec<(KeyCode, &Action, &str)> {
        let mut found: Vec<_> = self
            .overrides
            .iter()
            .filter(|((m, _), _)| *m == mode)
            .filter_map(|((_, key), o)| Some((*key, o.action.as_ref()?, o.name.as_str())))
            .collect();
        found.sort_by_key(|(key, _, _)| key_name(*key));
        found
    }

    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty()
    }
}

/// Parse a key name: a single character, or one of the named keys.
pub fn parse_key(name: &str) -> Option<KeyCode> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(KeyCode::Char(c));
    }
    Some(match name.to_ascii_lowercase().as_str() {
        "space" => KeyCode::Char(' '),
        "tab" => KeyCode::Tab,
        "s-tab" | "shift-tab" | "backtab" => KeyCode::BackTab,
        "enter" | "return" | "cr" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "backspace" | "bs" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        _ => return None,
    })
}

/// How a key is spelled in the help overlay.
pub fn key_name(key: KeyCode) -> String {
    match key {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "S-Tab".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Backspace => "BS".to_string(),
        KeyCode::Delete => "Del".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PgUp".to_string(),
        KeyCode::PageDown => "PgDn".to_string(),
        other => format!("{other:?}"),
    }
}

/// `None` for the literal action name "none", which unbinds a key.
fn parse_action(name: &str) -> Option<Option<Action>> {
    if name == "none" {
        return Some(None);
    }
    Some(Some(match name {
        // Navigation
        "focus_prev_column" => Action::FocusPrevColumn,
        "focus_next_column" => Action::FocusNextColumn,
        "select_prev_card" => Action::SelectPrevCard,
        "select_next_card" => Action::SelectNextCard,
        "cycle_next_card" => Action::CycleNextCard,
        "cycle_prev_card" => Action::CyclePrevCard,
        "jump_to_first_card" => Action::JumpToFirstCard,
        "jump_to_last_card" => Action::JumpToLastCard,
        "jump_to_first_card_global" => Action::JumpToFirstCardGlobal,
        "jump_to_last_card_global" => Action::JumpToLastCardGlobal,
        "jump_to_first_column" => Action::JumpToFirstColumn,
        "jump_to_last_column" => Action::JumpToLastColumn,
        // Cards
        "move_card_prev_column" => Action::MoveCardPrevColumn,
        "move_card_next_column" => Action::MoveCardNextColumn,
        "new_card" => Action::NewCard,
        "delete_card" => Action::DeleteCard,
        "edit_card_external" => Action::EditCardExternal,
        "edit_tags" => Action::EditTags,
        "edit_assignees" => Action::EditAssignees,
        "pick_priority" => Action::PickPriority,
        "move_to_column" => Action::MoveToColumn,
        "toggle_blocker" => Action::ToggleBlocker,
        "set_due_date" => Action::SetDueDate,
        "archive_card" => Action::ArchiveCard,
        "pipe_card" => Action::PipeCard,
        "open_card_detail" => Action::OpenCardDetail,
        // Detail panel
        "close_panel" => Action::ClosePanel,
        "detail_scroll_up" => Action::DetailScrollUp,
        "detail_scroll_down" => Action::DetailScrollDown,
        "detail_next_card" => Action::DetailNextCard,
        "detail_prev_card" => Action::DetailPrevCard,
        "toggle_detail_image" => Action::ToggleDetailImage,
        "detail_next_image" => Action::DetailNextImage,
        "detail_prev_image" => Action::DetailPrevImage,
        // Columns
        "toggle_hidden_columns" => Action::ToggleHiddenColumns,
        "toggle_focused_column_hidden" => Action::ToggleFocusedColumnHidden,
        "col_rename_selected" => Action::ColRenameSelected,
        "col_add_before" => Action::ColAddBefore,
        "col_remove_selected" => Action::ColRemoveSelected,
        "col_set_wip" => Action::ColSetWip,
        "enter_col_move_mode" => Action::EnterColMoveMode,
        "col_move_left" => Action::ColMoveLeft,
        "col_move_right" => Action::ColMoveRight,
        "col_move_first" => Action::ColMoveFirst,
        "col_move_last" => Action::ColMoveLast,
        // Search, filter, sort
        "start_sort" => Action::StartSort,
        "start_filter" => Action::StartFilter,
        "start_tag_filter" => Action::StartTagFilter,
        "start_assignee_filter" => Action::StartAssigneeFilter,
        "start_staleness_filter" => Action::StartStalenessFilter,
        "start_overdue_filter" => Action::StartOverdueFilter,
        "find_next" => Action::FindNext,
        "find_prev" => Action::FindPrev,
        "clear_filters" => Action::ClearFilters,
        // Everything else
        "undo" => Action::Undo,
        "repeat_last" => Action::RepeatLast,
        "toggle_compact_view" => Action::ToggleCompactView,
        "reload_board" => Action::ReloadBoard,
        "show_help" => Action::ShowHelp,
        "toggle_help_page" => Action::ToggleHelpPage,
        "show_metrics" => Action::ShowMetrics,
        "quit" => Action::Quit,
        // Minor modes
        "enter_goto_mode" => Action::EnterGotoMode,
        "enter_goto_column_mode" => Action::EnterGotoColumnMode,
        "enter_space_mode" => Action::EnterSpaceMode,
        "enter_column_mode" => Action::EnterColumnMode,
        "enter_filter_mode" => Action::EnterFilterMode,
        "enter_template_mode" => Action::EnterTemplateMode,
        "template_new" => Action::TemplateNew,
        "template_edit" => Action::TemplateEdit,
        "template_delete" => Action::TemplateDelete,
        "template_rename" => Action::TemplateRename,
        _ => return None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_keymap() {
        let cfg = KeyConfig::parse(
            r#"
            [normal]
            b = "find_next"
            B = "find_prev"
            S = "start_sort"
            s = "none"

            [space]
            q = "quit"
            "#,
        )
        .unwrap();

        let plain = KeyModifiers::NONE;
        assert_eq!(
            cfg.lookup(BindMode::Normal, KeyCode::Char('b'), plain),
            Some(&Some(Action::FindNext))
        );
        assert_eq!(
            cfg.lookup(BindMode::Normal, KeyCode::Char('s'), plain),
            Some(&None),
            "s should be explicitly unbound"
        );
        assert_eq!(
            cfg.lookup(BindMode::Space, KeyCode::Char('q'), plain),
            Some(&Some(Action::Quit))
        );
        assert_eq!(cfg.lookup(BindMode::Normal, KeyCode::Char('j'), plain), None);
    }

    #[test]
    fn modified_keys_keep_their_builtin_meaning() {
        let cfg = KeyConfig::parse("[normal]\nc = \"quit\"").unwrap();
        assert!(cfg
            .lookup(BindMode::Normal, KeyCode::Char('c'), KeyModifiers::CONTROL)
            .is_none());
        assert!(cfg
            .lookup(BindMode::Normal, KeyCode::Char('c'), KeyModifiers::NONE)
            .is_some());
    }

    #[test]
    fn shift_is_not_treated_as_a_modifier() {
        // Terminals report `B` as Char('B') with SHIFT; that must still match.
        let cfg = KeyConfig::parse("[normal]\nB = \"find_prev\"").unwrap();
        assert!(cfg
            .lookup(BindMode::Normal, KeyCode::Char('B'), KeyModifiers::SHIFT)
            .is_some());
    }

    #[test]
    fn named_keys_round_trip() {
        for name in ["tab", "s-tab", "enter", "esc", "space", "up", "pagedown"] {
            let key = parse_key(name).unwrap_or_else(|| panic!("{name} should parse"));
            assert!(!key_name(key).is_empty());
        }
        assert_eq!(parse_key("|"), Some(KeyCode::Char('|')));
        assert_eq!(parse_key("?"), Some(KeyCode::Char('?')));
    }

    #[test]
    fn finds_the_key_bound_to_an_action() {
        let cfg = KeyConfig::parse("[normal]\nb = \"find_next\"").unwrap();
        assert_eq!(
            cfg.key_for(BindMode::Normal, &Action::FindNext),
            Some(KeyCode::Char('b'))
        );
        assert_eq!(cfg.key_for(BindMode::Normal, &Action::FindPrev), None);
        assert_eq!(cfg.key_for(BindMode::Space, &Action::FindNext), None);
    }

    #[test]
    fn typos_are_reported_rather_than_ignored() {
        let unknown_mode = KeyConfig::parse("[nrmal]\nb = \"find_next\"").unwrap_err();
        assert!(unknown_mode.contains("nrmal"), "{unknown_mode}");

        let unknown_action = KeyConfig::parse("[normal]\nb = \"find_nxt\"").unwrap_err();
        assert!(unknown_action.contains("find_nxt"), "{unknown_action}");

        let unknown_key = KeyConfig::parse("[normal]\nhyper = \"find_next\"").unwrap_err();
        assert!(unknown_key.contains("hyper"), "{unknown_key}");

        assert!(KeyConfig::parse("[normal\nb =").is_err());
    }

    #[test]
    fn an_empty_or_absent_config_changes_nothing() {
        let cfg = KeyConfig::parse("").unwrap();
        assert!(cfg.is_empty());
        assert_eq!(
            cfg.lookup(BindMode::Normal, KeyCode::Char('q'), KeyModifiers::NONE),
            None
        );
    }
}
