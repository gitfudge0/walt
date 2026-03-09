use std::hash::Hash;

use eframe::egui::{
    self, Align, Align2, Button, Color32, Context, FontId, Id, Margin, Rect, Response, RichText,
    Sense, Stroke, Ui, Vec2,
};

#[derive(Clone, Copy)]
pub(crate) struct GuiPalette {
    pub(crate) background: Color32,
    pub(crate) surface: Color32,
    pub(crate) surface_alt: Color32,
    pub(crate) border: Color32,
    pub(crate) accent: Color32,
    pub(crate) highlight: Color32,
    pub(crate) text: Color32,
    pub(crate) muted: Color32,
    pub(crate) danger: Color32,
    pub(crate) success: Color32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GuiTextRole {
    AppEyebrow,
    Body,
    BodyMuted,
    ListItem,
    ListBadge,
    SectionLabel,
    MetaLabel,
    MetaValue,
    ActionLabel,
    PopupTitle,
    PopupBody,
    Toast,
    Hero,
}

pub(crate) struct GuiTypography;

impl GuiTypography {
    pub(crate) fn font_id(role: GuiTextRole) -> FontId {
        match role {
            GuiTextRole::Hero => FontId::monospace(22.0),
            GuiTextRole::Body => FontId::monospace(16.0),
            GuiTextRole::BodyMuted => FontId::monospace(16.0),
            GuiTextRole::ListItem => FontId::monospace(15.0),
            GuiTextRole::ActionLabel => FontId::monospace(14.0),
            GuiTextRole::PopupTitle => FontId::monospace(13.0),
            GuiTextRole::Toast => FontId::monospace(13.0),
            GuiTextRole::AppEyebrow
            | GuiTextRole::ListBadge
            | GuiTextRole::SectionLabel
            | GuiTextRole::MetaLabel
            | GuiTextRole::MetaValue
            | GuiTextRole::PopupBody => FontId::monospace(12.0),
        }
    }

    pub(crate) fn rich(
        role: GuiTextRole,
        text: impl Into<String>,
        palette: GuiPalette,
    ) -> RichText {
        Self::rich_color(role, text, Self::default_color(role, palette))
    }

    pub(crate) fn rich_color(
        role: GuiTextRole,
        text: impl Into<String>,
        color: Color32,
    ) -> RichText {
        RichText::new(text.into())
            .font(Self::font_id(role))
            .color(color)
    }

    fn default_color(role: GuiTextRole, palette: GuiPalette) -> Color32 {
        match role {
            GuiTextRole::AppEyebrow
            | GuiTextRole::BodyMuted
            | GuiTextRole::ListBadge
            | GuiTextRole::SectionLabel
            | GuiTextRole::MetaLabel => palette.muted,
            GuiTextRole::Toast
            | GuiTextRole::Body
            | GuiTextRole::ListItem
            | GuiTextRole::MetaValue
            | GuiTextRole::ActionLabel
            | GuiTextRole::PopupTitle
            | GuiTextRole::PopupBody
            | GuiTextRole::Hero => palette.text,
        }
    }
}

pub(crate) struct GuiChrome;

impl GuiChrome {
    pub(crate) fn button(
        label: impl Into<String>,
        role: GuiTextRole,
        palette: GuiPalette,
    ) -> Button<'static> {
        Self::button_colored(label, role, palette.text, palette)
    }

    pub(crate) fn button_colored(
        label: impl Into<String>,
        role: GuiTextRole,
        color: Color32,
        palette: GuiPalette,
    ) -> Button<'static> {
        Button::new(GuiTypography::rich_color(role, label, color))
            .fill(palette.background)
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(0.0)
    }

    pub(crate) fn close_button(palette: GuiPalette) -> Button<'static> {
        Button::new(GuiTypography::rich_color(
            GuiTextRole::ActionLabel,
            "×",
            palette.muted,
        ))
        .fill(palette.background)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(0.0)
        .min_size(Vec2::new(28.0, 28.0))
    }

    pub(crate) fn popup_frame(palette: GuiPalette) -> egui::Frame {
        egui::Frame::new()
            .fill(palette.background)
            .stroke(Stroke::new(1.0, palette.border))
            .inner_margin(Margin::same(0))
    }

    pub(crate) fn panel_frame(palette: GuiPalette, margin: i8) -> egui::Frame {
        egui::Frame::new()
            .fill(palette.background)
            .inner_margin(Margin::same(margin))
    }

    pub(crate) fn rule(ui: &mut Ui, palette: GuiPalette, height: f32) {
        let width = ui.available_width().max(1.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
        let y = rect.center().y;
        ui.painter().line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            Stroke::new(1.0, palette.border),
        );
    }
}

pub(crate) fn interactive_row<R>(
    ui: &mut Ui,
    id_source: impl Hash,
    height: f32,
    add_contents: impl FnOnce(&mut Ui, Rect, &Response) -> R,
) -> (Response, R) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    let id = ui.make_persistent_id(id_source);
    let inner_rect = rect.shrink2(egui::vec2(0.0, 1.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner_rect)
            .layout(egui::Layout::left_to_right(Align::Center)),
    );
    let preview_response = ui.interact(rect, id.with("hover"), Sense::hover());
    let result = add_contents(&mut child, inner_rect, &preview_response);
    let response = ui.interact(rect, id, Sense::click());
    (response, result)
}

pub(crate) fn show_popup_shell(
    ctx: &Context,
    id_source: impl Hash + Copy,
    title: &str,
    palette: GuiPalette,
    default_width: Option<f32>,
    add_body: impl FnOnce(&mut Ui) -> bool,
) -> bool {
    let mut close_requested = false;
    let mut window = egui::Window::new("")
        .id(Id::new(id_source))
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .frame(GuiChrome::popup_frame(palette));

    if let Some(width) = default_width {
        window = window.default_width(width);
    }

    window.show(ctx, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);

        egui::Frame::new()
            .fill(palette.background)
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(GuiTypography::rich(GuiTextRole::PopupTitle, title, palette));
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(GuiChrome::close_button(palette)).clicked() {
                            close_requested = true;
                        }
                    });
                });
            });

        GuiChrome::rule(ui, palette, 8.0);

        egui::Frame::new()
            .fill(palette.background)
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                if add_body(ui) {
                    close_requested = true;
                }
            });
    });

    !close_requested
}

#[cfg(test)]
mod tests {
    use super::{GuiTextRole, GuiTypography};

    #[test]
    fn semantic_type_sizes_match_expected_scale() {
        assert_eq!(GuiTypography::font_id(GuiTextRole::Hero).size, 22.0);
        assert_eq!(GuiTypography::font_id(GuiTextRole::Body).size, 16.0);
        assert_eq!(GuiTypography::font_id(GuiTextRole::PopupTitle).size, 13.0);
        assert_eq!(GuiTypography::font_id(GuiTextRole::MetaValue).size, 12.0);
    }

    #[test]
    fn related_small_roles_share_size() {
        let label = GuiTypography::font_id(GuiTextRole::MetaLabel).size;
        let popup = GuiTypography::font_id(GuiTextRole::PopupBody).size;
        let badge = GuiTypography::font_id(GuiTextRole::ListBadge).size;
        assert_eq!(label, popup);
        assert_eq!(popup, badge);
    }
}
