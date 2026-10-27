//! The compositor binds: modifier, modifier state and keysym in, an action
//! out.

use smithay::input::keyboard::{Keysym, ModifiersState, keysyms};
use tetsu_comp::engine::Action;
use tetsu_comp::input::{Bound, ModKey, bound_action};

fn mods(ctrl: bool, alt: bool, shift: bool, logo: bool) -> ModifiersState {
    ModifiersState {
        ctrl,
        alt,
        shift,
        logo,
        ..ModifiersState::default()
    }
}

/// The modifiers that make the bind modifier, plus Shift when asked.
fn with(mod_key: ModKey, shift: bool) -> ModifiersState {
    match mod_key {
        ModKey::Super => mods(false, false, shift, true),
        ModKey::CtrlAlt => mods(true, true, shift, false),
    }
}

fn sym(raw: u32) -> Keysym {
    Keysym::new(raw)
}

fn bound(mod_key: ModKey, shift: bool, raw: u32) -> Option<Bound> {
    bound_action(mod_key, &with(mod_key, shift), sym(raw))
}

fn engine(action: Action) -> Option<Bound> {
    Some(Bound::Engine(action))
}

#[test]
fn every_bind_works_with_either_modifier() {
    let digits = [
        keysyms::KEY_1,
        keysyms::KEY_2,
        keysyms::KEY_3,
        keysyms::KEY_4,
        keysyms::KEY_5,
        keysyms::KEY_6,
        keysyms::KEY_7,
        keysyms::KEY_8,
        keysyms::KEY_9,
    ];
    for mod_key in [ModKey::Super, ModKey::CtrlAlt] {
        let table = [
            (false, keysyms::KEY_j, Action::FocusNext),
            (false, keysyms::KEY_k, Action::FocusPrev),
            (true, keysyms::KEY_j, Action::SwapNext),
            (true, keysyms::KEY_k, Action::SwapPrev),
            (true, keysyms::KEY_Return, Action::PromoteMaster),
            (false, keysyms::KEY_h, Action::RatioDelta(-1)),
            (false, keysyms::KEY_l, Action::RatioDelta(1)),
            (false, keysyms::KEY_space, Action::CycleLayout),
            (false, keysyms::KEY_f, Action::ToggleFloating),
            (false, keysyms::KEY_q, Action::Close),
        ];
        for (shift, raw, action) in table {
            assert_eq!(
                bound(mod_key, shift, raw),
                engine(action),
                "{mod_key:?} {raw:#x}"
            );
        }
        for (i, raw) in digits.iter().enumerate() {
            let n = u8::try_from(i + 1).unwrap();
            assert_eq!(
                bound(mod_key, false, *raw),
                engine(Action::FocusWorkspace(n))
            );
            assert_eq!(
                bound(mod_key, true, *raw),
                engine(Action::MoveToWorkspace(n))
            );
        }
    }
}

#[test]
fn shifted_letters_and_raw_digits_both_match() {
    let shift = with(ModKey::Super, true);
    // The modified sym of Shift+J is an upper-case J.
    assert_eq!(
        bound_action(ModKey::Super, &shift, sym(keysyms::KEY_J)),
        engine(Action::SwapNext)
    );
    // The glue hands over the unshifted digit; the shifted symbol of the
    // layout (`exclam`) is not a bind.
    assert_eq!(
        bound_action(ModKey::Super, &shift, sym(keysyms::KEY_1)),
        engine(Action::MoveToWorkspace(1))
    );
    assert_eq!(
        bound_action(ModKey::Super, &shift, sym(keysyms::KEY_exclam)),
        None
    );
}

#[test]
fn the_wrong_modifier_binds_nothing() {
    // Super binds need Super; Ctrl+Alt binds need Ctrl+Alt.
    assert_eq!(
        bound_action(
            ModKey::Super,
            &with(ModKey::CtrlAlt, false),
            sym(keysyms::KEY_j)
        ),
        None
    );
    assert_eq!(
        bound_action(
            ModKey::CtrlAlt,
            &with(ModKey::Super, false),
            sym(keysyms::KEY_j)
        ),
        None
    );
    assert_eq!(
        bound_action(
            ModKey::CtrlAlt,
            &mods(true, false, false, false),
            sym(keysyms::KEY_j)
        ),
        None
    );
    assert_eq!(
        bound_action(
            ModKey::Super,
            &mods(false, false, false, false),
            sym(keysyms::KEY_j)
        ),
        None
    );
}

#[test]
fn the_old_binds_still_work_with_any_modifier_choice() {
    let ctrl_alt = mods(true, true, false, false);
    for mod_key in [ModKey::Super, ModKey::CtrlAlt] {
        assert_eq!(
            bound_action(mod_key, &ctrl_alt, Keysym::BackSpace),
            Some(Bound::Quit)
        );
        assert_eq!(
            bound_action(mod_key, &ctrl_alt, sym(keysyms::KEY_XF86Switch_VT_1)),
            Some(Bound::SwitchVt(1))
        );
        assert_eq!(
            bound_action(mod_key, &ctrl_alt, sym(keysyms::KEY_XF86Switch_VT_12)),
            Some(Bound::SwitchVt(12))
        );
        assert_eq!(
            bound_action(
                mod_key,
                &mods(false, false, false, false),
                Keysym::BackSpace
            ),
            None
        );
    }
}

#[test]
fn unbound_keys_give_nothing() {
    for mod_key in [ModKey::Super, ModKey::CtrlAlt] {
        assert_eq!(bound(mod_key, false, keysyms::KEY_x), None);
        assert_eq!(bound(mod_key, false, keysyms::KEY_Return), None);
        assert_eq!(bound(mod_key, true, keysyms::KEY_q), None);
        assert_eq!(bound(mod_key, true, keysyms::KEY_space), None);
        assert_eq!(bound(mod_key, false, keysyms::KEY_0), None);
    }
    assert_eq!(
        bound_action(
            ModKey::Super,
            &ModifiersState::default(),
            sym(keysyms::KEY_a)
        ),
        None
    );
}
