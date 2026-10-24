use super::PrintWindow;
use slint::{ComponentHandle as _, Model as _, ModelRc, VecModel};

pub(super) fn canonical(value: &str) -> Option<String> {
    let value: String = value
        .trim()
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-')
        .map(|ch| ch.to_ascii_uppercase())
        .collect();
    (value.len() == 8
        && value
            .chars()
            .all(|ch| "23456789ABCDEFGHJKLMNPQRSTUVWXYZ".contains(ch)))
    .then_some(value)
}

pub(super) fn clear(ui: &PrintWindow) {
    ui.set_code_parts(ModelRc::new(VecModel::from(
        vec![slint::SharedString::default(); 8],
    )));
    ui.set_code_focus_index(0);
}

fn edit(ui: &PrintWindow, index: i32, value: &str) {
    let Ok(start) = usize::try_from(index) else {
        return;
    };
    if start >= 8 {
        return;
    }
    let value: String = value
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-')
        .map(|ch| ch.to_ascii_uppercase())
        .collect();
    let model = ui.get_code_parts();
    if !value.chars().all(|ch| ch.is_ascii_alphanumeric()) || value.len() > 8 - start {
        model.set_row_data(start, slint::SharedString::default());
        ui.set_message("配对码为 8 位字母和数字，请检查粘贴内容。".into());
        return;
    }
    if value.is_empty() {
        model.set_row_data(start, slint::SharedString::default());
        return;
    }
    for (offset, ch) in value.chars().enumerate() {
        model.set_row_data(start + offset, ch.to_string().into());
    }
    let focus = i32::try_from((start + value.len()).min(7)).unwrap_or(7);
    ui.set_code_focus_index(focus);
    ui.set_message(slint::SharedString::default());
}

pub(super) fn bind(ui: &PrintWindow) {
    clear(ui);
    ui.on_pairing_valid(|code| canonical(code.as_str()).is_some());
    let weak = ui.as_weak();
    ui.on_code_edited(move |index, text| {
        if let Some(ui) = weak.upgrade() {
            edit(&ui, index, text.as_str());
        }
    });
    let weak = ui.as_weak();
    ui.on_code_move(move |index, delta| {
        if let Some(ui) = weak.upgrade() {
            ui.set_code_focus_index((index + delta).clamp(0, 7));
        }
    });
    let weak = ui.as_weak();
    ui.on_code_backspace(move |index| {
        if let Some(ui) = weak.upgrade() {
            let index = (index - 1).clamp(0, 7);
            if let Ok(row) = usize::try_from(index) {
                ui.get_code_parts()
                    .set_row_data(row, slint::SharedString::default());
            }
            ui.set_code_focus_index(index);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pasted_codes_use_the_same_closed_alphabet_without_ever_becoming_a_token() {
        assert_eq!(canonical(" abcd-2345 ").as_deref(), Some("ABCD2345"));
        for bad in ["", "ABCD1234", "ABCD23456", "ABCD23IO", "ABCD🚀2345"] {
            assert!(canonical(bad).is_none());
        }
    }
}
