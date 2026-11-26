//! Control spacing and checkbox styling for the desktop.

use std::cell::Cell;

use adw::prelude::*;
use gtk::{gdk, pango};

use crate::checkbox::{self, CORNER_RADIUS, OUTLINE_OPACITY};

const COMPACT_CONTROLS: &str = "
button, entry:not(.flat), searchentry {
    min-height: 22px;
    padding: 3px 6px;
}
button.image-button,
menubutton.image-button > button,
splitbutton.image-button > button,
windowcontrols > button {
    min-width: 20px;
    padding: 3px 4px;
}
windowcontrols > button > image { padding: 2px 3px; }
headerbar > windowhandle > box { padding: 7px; }
headerbar.default-decoration > windowhandle > box { padding: 4px; }
headerbar:not(.default-decoration) { min-height: 42px; }
checkbutton, checkbutton.text-button { padding: 1px 2px; }
spinbutton { min-height: 28px; }
spinbutton:not(.vertical) > text { padding: 3px 5px; }
spinbutton:not(.vertical) > button {
    min-height: 16px;
    padding: 0 4px;
}
calendar > grid > label.day-number { padding: 2px 3px; }
dialog.alert .message-area { padding-top: 24px; }
dialog.alert.short .message-area { padding-top: 18px; }
dialog.alert .response-area > button { padding: 5px 8px; }
list > row:not(.expander), listview > row:not(.expander) { padding: 1px 2px; }
.navigation-sidebar > row {
    min-height: 28px;
    padding: 2px 5px;
}
row > box.header {
    margin-left: 8px;
    margin-right: 8px;
    min-height: 42px;
}
row > box.header > box.title {
    margin-bottom: 4px;
    margin-top: 4px;
}
popover:not(.menu) > contents { padding: 6px; }
popover.menu > contents > stack > box,
popover.menu > contents > scrolledwindow > viewport > stack > box { padding: 5px; }
popover.menu modelbutton,
popover.menu contents > list > row,
popover.menu contents > listview > row,
popover.menu scrolledwindow > viewport > list > row,
popover.menu scrolledwindow > viewport > listview > row {
    min-height: 22px;
    padding: 3px 8px;
}
";

pub(crate) fn install() {
    thread_local! {
        static INSTALLED: Cell<bool> = const { Cell::new(false) };
    }
    let Some(display) = gdk::Display::default() else { return };
    if INSTALLED.replace(true) {
        return;
    }
    let provider = gtk::CssProvider::new();
    let icon = checkbox::check_icon();
    load(&provider, &icon);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let settings = gtk::Settings::for_display(&display);
    let font_provider = provider.clone();
    let font_icon = icon.clone();
    settings.connect_gtk_font_name_notify(move |_| load(&font_provider, &font_icon));
    settings.connect_gtk_xft_dpi_notify(move |_| load(&provider, &icon));
}

fn load(provider: &gtk::CssProvider, icon: &str) {
    // Markdown draws its box over a bold monospace list prefix. Measure that font cell.
    let layout = gtk::Label::new(None).create_pango_layout(Some("- "));
    let attributes = pango::AttrList::new();
    attributes.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
    attributes.insert(pango::AttrString::new_family("monospace"));
    layout.set_attributes(Some(&attributes));
    let cell = layout.index_to_pos(0);
    #[allow(clippy::cast_precision_loss)]
    let (advance, height) =
        (cell.width() as f32 / pango::SCALE as f32, cell.height() as f32 / pango::SCALE as f32);
    let side = checkbox::side(height);
    // The text follows two cells, and the box is centered on the first one.
    let left = ((advance - side) / 2.0).round();
    let gap = (2.0 * advance - left - side).round().max(0.0);
    let column_padding = (gap - 2.0).max(3.0);
    let indicator_padding = (column_padding / 2.0).floor();
    let title_padding = column_padding - indicator_padding;
    let border = checkbox::border(side);
    let hover_opacity = OUTLINE_OPACITY + 0.05;
    let radius = side * CORNER_RADIUS;
    provider.load_from_string(&format!(
        r#"
{COMPACT_CONTROLS}
button.zib-task-expander {{ min-height: 20px; min-width: 16px; }}
button.zib-task-expander,
button.zib-work-title {{ padding: 2px {column_padding}px; }}
button.zib-work-title:dir(ltr) {{ padding-left: {title_padding}px; }}
button.zib-work-title:dir(rtl) {{ padding-right: {title_padding}px; }}
.zib-work-row-heading {{ border-radius: 9px; }}
entry.zib-work-addition:dir(ltr),
.zib-work-row-heading:dir(ltr) {{ padding-left: {column_padding}px; }}
entry.zib-work-addition:dir(rtl),
.zib-work-row-heading:dir(rtl) {{ padding-right: {column_padding}px; }}
.zib-work-row-heading:hover {{ background-color: alpha(currentColor, 0.07); }}
.zib-work-row-heading:active {{ background-color: alpha(currentColor, 0.16); }}
@media (prefers-contrast: more) {{
    .zib-work-row-heading:active,
    .zib-work-row-heading:hover {{ box-shadow: inset 0 0 0 1px alpha(currentColor, 0.5); }}
}}
.zib-work-row-heading button.zib-work-title {{
    background: transparent;
    box-shadow: none;
}}
.zib-work-row-indicator:dir(ltr) {{ padding: 2px {indicator_padding}px 2px 0; }}
.zib-work-row-indicator:dir(rtl) {{ padding: 2px 0 2px {indicator_padding}px; }}
image.zib-work-row-indicator {{ -gtk-icon-size: {side}px; }}
checkbutton {{ border-spacing: {gap}px; }}
.zib-work-column-content:dir(ltr) {{ margin-left: {column_padding}px; }}
.zib-work-column-content:dir(rtl) {{ margin-right: {column_padding}px; }}
.zib-work-row .zib-work-details:dir(ltr) {{ margin-left: {title_padding}px; }}
.zib-work-row .zib-work-details:dir(rtl) {{ margin-right: {title_padding}px; }}
checkbutton > check {{
    -gtk-icon-size: {side}px;
    border-radius: {radius}px;
    min-height: {side}px;
    min-width: {side}px;
    padding: 0;
}}
checkbutton > check:not(:checked):not(:indeterminate):not(:active) {{
    box-shadow: inset 0 0 0 {border}px rgb(from currentColor r g b / {OUTLINE_OPACITY});
}}
checkbutton > check:not(:checked):not(:indeterminate):not(:active):hover {{
    box-shadow: inset 0 0 0 {border}px rgb(from currentColor r g b / {hover_opacity});
}}
checkbutton > check:checked:not(:indeterminate) {{
    -gtk-icon-source: url("data:image/png;base64,{icon}");
}}
"#
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_controls_use_compact_heights() {
        crate::test_support::init_gtk();
        gtk::Settings::default().unwrap().set_gtk_font_name(Some("Adwaita Sans 11"));
        let controls: [gtk::Widget; 5] = [
            gtk::Button::with_label("Save").upcast(),
            gtk::Button::from_icon_name("view-more-symbolic").upcast(),
            gtk::Entry::new().upcast(),
            gtk::SearchEntry::new().upcast(),
            gtk::DropDown::from_strings(&["First", "Second"]).upcast(),
        ];
        for control in controls {
            let height = control.measure(gtk::Orientation::Vertical, -1).1;
            assert!((24..=30).contains(&height), "{} height: {height}", control.css_name());
        }
    }
}
