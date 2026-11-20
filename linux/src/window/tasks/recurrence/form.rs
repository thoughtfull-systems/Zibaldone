//! Native controls for the date-only recurrence rules understood by the core.

use std::rc::Rc;

use adw::prelude::*;
use chrono::{Datelike, NaiveDate};
use zib_core::recurrence::{self, Frequency, Mode, Pattern, Rule, Weekday};
use zib_core::{Error, Result};

use super::super::controls::{connect_calendar_pick, select_date};

// The choices follow calendar order.
const DAYS: [(Weekday, &str); 7] = [
    (Weekday::Sun, "Sun"),
    (Weekday::Mon, "Mon"),
    (Weekday::Tue, "Tue"),
    (Weekday::Wed, "Wed"),
    (Weekday::Thu, "Thu"),
    (Weekday::Fri, "Fri"),
    (Weekday::Sat, "Sat"),
];
pub(super) const ORDINALS: [(i8, &str); 6] =
    [(1, "First"), (2, "Second"), (3, "Third"), (4, "Fourth"), (5, "Fifth"), (-1, "Last")];

pub(super) struct RuleForm {
    pub rows: gtk::Box,
    calendar: CalendarFilters,
    count: gtk::SpinButton,
    due: Offset,
    ending: gtk::DropDown,
    frequency: gtk::DropDown,
    interval: gtk::SpinButton,
    mode: gtk::DropDown,
    on_date: gtk::Box,
    original: Rule,
    original_pattern: Pattern,
    preview: gtk::Label,
    scheduled: Offset,
    start: gtk::Entry,
    until: gtk::Entry,
    updating: bool,
}

impl RuleForm {
    pub(super) fn new(rule: &Rule, updating: bool) -> Result<Rc<Self>> {
        let pattern: Pattern = rule.pattern.parse()?;
        let fields = grid();
        let mode = gtk::DropDown::from_strings(&[
            "Keep every occurrence",
            "Jump when resolved",
            "Repeat after completion",
        ]);
        mode.set_selected(match rule.mode {
            Mode::Keep => 0,
            Mode::Jump => 1,
            Mode::Interval => 2,
        });
        let interval = spin(1.0, f64::from(u32::MAX), f64::from(pattern.interval));
        accessible(&interval, "Interval");
        interval.set_tooltip_text(Some("Repeat interval"));
        let frequency = gtk::DropDown::from_strings(&["Days", "Weeks", "Months", "Years"]);
        accessible(&frequency, "Frequency");
        frequency.set_selected(match pattern.frequency {
            Frequency::Daily => 0,
            Frequency::Weekly => 1,
            Frequency::Monthly => 2,
            Frequency::Yearly => 3,
        });
        field(&fields, 1, "Every", &pair(&interval, &frequency));
        let (start_row, start) = calendar_input(&rule.start.to_string(), "Start date");
        field(&fields, 0, "Start date", &start_row);

        let calendar = CalendarFilters::new(&pattern, rule.start);
        fields.attach(&calendar.grid, 0, 2, 2, 1);

        let ending = gtk::DropDown::from_strings(&["Never", "Through a date", "After occurrences"]);
        ending.set_selected(match (pattern.count, pattern.until) {
            (Some(_), _) => 2,
            (_, Some(_)) => 1,
            _ => 0,
        });
        field(&fields, 3, "Ends", &ending);
        let (on_date, until) =
            calendar_input(&pattern.until.unwrap_or(rule.start).to_string(), "End date");
        let count = spin(1.0, f64::from(u32::MAX), f64::from(pattern.count.unwrap_or(5)));
        accessible(&count, "Occurrence count");
        count.set_tooltip_text(Some("Number of occurrences"));
        fields.attach(&on_date, 1, 4, 1, 1);
        fields.attach(&count, 1, 4, 1, 1);
        field(&fields, 5, "Mode", &mode);

        let scheduled = Offset::new(rule.scheduled, "Scheduled offset");
        let due = Offset::new(rule.due, "Due offset");
        field(&fields, 6, "Scheduled", &scheduled.row);
        field(&fields, 7, "Due", &due.row);

        let scroll = gtk::ScrolledWindow::builder()
            .child(&fields)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(490)
            .propagate_natural_height(true)
            .build();
        let preview = gtk::Label::builder().wrap(true).max_width_chars(44).xalign(0.0).build();
        preview.add_css_class("caption");
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 10);
        rows.append(&scroll);
        rows.append(&preview);
        Ok(Rc::new(Self {
            rows,
            calendar,
            count,
            due,
            ending,
            frequency,
            interval,
            mode,
            on_date,
            original: rule.clone(),
            original_pattern: pattern,
            preview,
            scheduled,
            start,
            until,
            updating,
        }))
    }

    fn frequency(&self) -> Frequency {
        match self.frequency.selected() {
            0 => Frequency::Daily,
            1 => Frequency::Weekly,
            2 => Frequency::Monthly,
            _ => Frequency::Yearly,
        }
    }

    pub(super) fn read(&self) -> Result<Rule> {
        let start =
            self.start.text().parse().map_err(|_| Error::Space("Choose a start date".into()))?;
        let mode = match self.mode.selected() {
            0 => Mode::Keep,
            1 => Mode::Jump,
            _ => Mode::Interval,
        };
        let mut pattern = self.original_pattern.clone();
        pattern.frequency = self.frequency();
        pattern.interval = number(&self.interval)?;
        pattern.count = (self.ending.selected() == 2).then(|| number(&self.count)).transpose()?;
        pattern.until = (self.ending.selected() == 1)
            .then(|| {
                self.until.text().parse().map_err(|_| Error::Space("Choose an end date".into()))
            })
            .transpose()?;
        self.calendar.apply(&mut pattern, start, mode)?;
        let unchanged = self.updating
            && mode == self.original.mode
            && start == self.original.start
            && pattern.to_string() == self.original_pattern.to_string();
        let pattern = if unchanged {
            self.original.pattern.clone()
        } else {
            if pattern.frequency == Frequency::Weekly {
                pattern.week_start = Weekday::Sun;
            }
            pattern.to_string()
        };
        let rule =
            Rule { mode, start, pattern, scheduled: self.scheduled.read(), due: self.due.read() };
        if self.updating
            && (rule.mode, rule.start, &rule.pattern)
                != (self.original.mode, self.original.start, &self.original.pattern)
            && start < recurrence::today()
        {
            return Err(Error::Space(
                "Choose today or a future start date for the changed rule".into(),
            ));
        }
        Ok(rule)
    }

    pub(super) fn watch(self: &Rc<Self>, dialog: &adw::AlertDialog) {
        let update = Rc::new({
            let form = Rc::downgrade(self);
            let dialog = dialog.downgrade();
            move || {
                let (Some(form), Some(dialog)) = (form.upgrade(), dialog.upgrade()) else { return };
                form.calendar.update(form.frequency(), form.mode.selected() != 2);
                form.on_date.set_visible(form.ending.selected() == 1);
                form.count.set_visible(form.ending.selected() == 2);
                for offset in [&form.scheduled, &form.due] {
                    offset.days.set_sensitive(offset.enabled.is_active());
                }
                let dates = form.read().and_then(|rule| rule.preview());
                dialog.set_response_enabled("save", dates.is_ok());
                form.preview.set_text(&match dates {
                    Ok(dates) => {
                        let text: Vec<_> = dates.into_iter().map(|date| date.to_string()).collect();
                        format!("Occurrences · {}", text.join(", "))
                    }
                    Err(error) => error.to_string(),
                });
            }
        });
        for entry in [&self.start, &self.until] {
            let update = update.clone();
            entry.connect_changed(move |_| update());
        }
        for choice in [
            &self.mode,
            &self.frequency,
            &self.calendar.month,
            &self.calendar.position,
            &self.calendar.monthly_day,
            &self.calendar.monthly_pattern,
            &self.ending,
        ] {
            let update = update.clone();
            choice.connect_selected_notify(move |_| update());
        }
        for spin in [
            &self.interval,
            &self.count,
            &self.calendar.month_day,
            &self.scheduled.days,
            &self.due.days,
        ] {
            let update = update.clone();
            spin.connect_value_changed(move |_| update());
        }
        for day in &self.calendar.weekdays {
            let update = update.clone();
            day.connect_toggled(move |_| update());
        }
        for check in [&self.calendar.saved_pattern, &self.scheduled.enabled, &self.due.enabled] {
            let update = update.clone();
            check.connect_toggled(move |_| update());
        }
        update();
    }
}

struct CalendarFilters {
    grid: gtk::Grid,
    month: gtk::DropDown,
    month_label: gtk::Label,
    month_day: gtk::SpinButton,
    monthly_day: gtk::DropDown,
    monthly_pattern: gtk::DropDown,
    ordinal_row: gtk::Box,
    position: gtk::DropDown,
    saved_pattern: gtk::CheckButton,
    saved_frequency: Option<Frequency>,
    when: gtk::Label,
    weekdays: [gtk::ToggleButton; 7],
    weekly_label: gtk::Label,
    weekly_row: gtk::Box,
}

impl CalendarFilters {
    fn new(pattern: &Pattern, start: NaiveDate) -> Self {
        let calendar = grid();
        let saved_pattern = gtk::CheckButton::with_label("Keep saved calendar pattern");
        let saved_frequency = has_combined_filters(pattern).then_some(pattern.frequency);
        saved_pattern.set_active(saved_frequency.is_some());
        calendar.attach(&saved_pattern, 1, 0, 1, 1);

        let weekly_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let weekdays = DAYS.map(|(day, label)| {
            let button = gtk::ToggleButton::with_label(label);
            button.set_active(
                pattern.frequency == Frequency::Weekly && pattern.weekdays.contains(&(None, day)),
            );
            weekly_row.append(&button);
            button
        });
        weekly_row.set_tooltip_text(Some("Uses the start date’s weekday when no day is selected"));
        let weekly_label = field(&calendar, 1, "Repeat on", &weekly_row);

        let month = gtk::DropDown::from_strings(&[
            "Start month",
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ]);
        if pattern.frequency == Frequency::Yearly {
            month.set_selected(pattern.month.unwrap_or(0));
        }
        let month_label = field(&calendar, 2, "Month", &month);
        let monthly_pattern = gtk::DropDown::from_strings(&[
            "Same day as start",
            "Day of the month",
            "Weekday of the month",
        ]);
        let (position, day) = pattern.weekdays.first().copied().unwrap_or((None, start.weekday()));
        monthly_pattern.set_selected(if position.is_some() {
            2
        } else {
            u32::from(pattern.month_day.is_some())
        });
        let when = field(&calendar, 3, "Repeat on", &monthly_pattern);
        let month_day = spin(1.0, 31.0, f64::from(pattern.month_day.unwrap_or(start.day())));
        accessible(&month_day, "Day of the month");
        calendar.attach(&month_day, 1, 4, 1, 1);
        let positions = gtk::DropDown::from_strings(&ORDINALS.map(|(_, label)| label));
        accessible(&positions, "Weekday position");
        positions.set_selected(
            position.map_or(0, |n| if n == -1 { 5 } else { u32::try_from(n - 1).unwrap_or(0) }),
        );
        let monthly_day = gtk::DropDown::from_strings(&[
            "Sunday",
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
        ]);
        accessible(&monthly_day, "Weekday");
        monthly_day.set_selected(day.num_days_from_sunday());
        let ordinal_row = pair(&positions, &monthly_day);
        calendar.attach(&ordinal_row, 1, 4, 1, 1);
        Self {
            grid: calendar,
            month,
            month_label,
            month_day,
            monthly_day,
            monthly_pattern,
            ordinal_row,
            position: positions,
            saved_pattern,
            saved_frequency,
            when,
            weekdays,
            weekly_label,
            weekly_row,
        }
    }

    fn keep_saved(&self, frequency: Frequency) -> bool {
        self.saved_frequency == Some(frequency) && self.saved_pattern.is_active()
    }

    fn update(&self, frequency: Frequency, calendar: bool) {
        self.grid.set_visible(
            calendar && (frequency != Frequency::Daily || self.saved_frequency == Some(frequency)),
        );
        self.saved_pattern.set_visible(self.saved_frequency == Some(frequency));
        let editable = !self.keep_saved(frequency);
        let weekly = editable && frequency == Frequency::Weekly;
        self.weekly_label.set_visible(weekly);
        self.weekly_row.set_visible(weekly);
        let yearly = editable && frequency == Frequency::Yearly;
        self.month.set_visible(yearly);
        self.month_label.set_visible(yearly);
        let monthly = yearly || editable && frequency == Frequency::Monthly;
        self.when.set_visible(monthly);
        self.monthly_pattern.set_visible(monthly);
        self.month_day.set_visible(monthly && self.monthly_pattern.selected() == 1);
        self.ordinal_row.set_visible(monthly && self.monthly_pattern.selected() == 2);
    }

    fn apply(&self, pattern: &mut Pattern, start: NaiveDate, mode: Mode) -> Result<()> {
        if mode != Mode::Interval && self.keep_saved(pattern.frequency) {
            return Ok(());
        }
        pattern.month = None;
        pattern.month_day = None;
        pattern.weekdays.clear();
        if mode == Mode::Interval {
            return Ok(());
        }
        match pattern.frequency {
            Frequency::Daily => {}
            Frequency::Weekly => {
                pattern.weekdays = self
                    .weekdays
                    .iter()
                    .zip(DAYS)
                    .filter_map(|(button, (day, _))| button.is_active().then_some((None, day)))
                    .collect();
            }
            Frequency::Monthly | Frequency::Yearly => {
                let selection = self.monthly_pattern.selected();
                if pattern.frequency == Frequency::Yearly {
                    pattern.month = match self.month.selected() {
                        0 if selection == 0 => None,
                        0 => Some(start.month()),
                        month => Some(month),
                    };
                }
                if selection == 1 {
                    pattern.month_day = Some(number(&self.month_day)?);
                } else if selection == 2 {
                    let position = ORDINALS.get(self.position.selected() as usize);
                    let day = DAYS.get(self.monthly_day.selected() as usize);
                    let (Some(&(position, _)), Some(&(day, _))) = (position, day) else {
                        return Err(Error::Space("Choose a weekday and position".into()));
                    };
                    pattern.weekdays.push((Some(position), day));
                }
            }
        }
        Ok(())
    }
}

fn has_combined_filters(pattern: &Pattern) -> bool {
    let weekdays = !pattern.weekdays.is_empty();
    let ordinal = pattern.weekdays.len() == 1 && pattern.weekdays[0].0.is_some();
    match pattern.frequency {
        Frequency::Daily => weekdays || pattern.month.is_some() || pattern.month_day.is_some(),
        Frequency::Weekly => pattern.month.is_some() || pattern.month_day.is_some(),
        Frequency::Monthly | Frequency::Yearly => {
            (weekdays && (!ordinal || pattern.month_day.is_some()))
                || (pattern.frequency == Frequency::Monthly && pattern.month.is_some())
                || (pattern.frequency == Frequency::Yearly
                    && pattern.month.is_none()
                    && pattern.month_day.is_some())
        }
    }
}

struct Offset {
    row: gtk::Box,
    enabled: gtk::CheckButton,
    days: gtk::SpinButton,
}

impl Offset {
    fn new(days: Option<i32>, label: &str) -> Self {
        let enabled = gtk::CheckButton::with_label("Use offset");
        accessible(&enabled, &format!("Use {}", label.to_lowercase()));
        enabled.set_tooltip_text(Some("Use the task’s date when unchecked"));
        enabled.set_active(days.is_some());
        let days = spin(f64::from(i32::MIN), f64::from(i32::MAX), f64::from(days.unwrap_or(0)));
        accessible(&days, label);
        days.set_tooltip_text(Some("Days before or after the occurrence date"));
        let input = pair(&days, &gtk::Label::new(Some("days")));
        Self { row: pair(&enabled, &input), enabled, days }
    }

    fn read(&self) -> Option<i32> {
        self.enabled.is_active().then(|| self.days.value_as_int())
    }
}

fn calendar_input(date: &str, name: &str) -> (gtk::Box, gtk::Entry) {
    let entry =
        gtk::Entry::builder().text(date).width_chars(11).placeholder_text("YYYY-MM-DD").build();
    accessible(&entry, name);
    let button = gtk::MenuButton::builder().icon_name("x-office-calendar-symbolic").build();
    let label = format!("Choose {}", name.to_lowercase());
    accessible(&button, &label);
    button.set_tooltip_text(Some(&label));
    let calendar = gtk::Calendar::new();
    let popup = gtk::Popover::new();
    popup.set_child(Some(&calendar));
    button.set_popover(Some(&popup));
    let input = entry.downgrade();
    let picker = calendar.downgrade();
    popup.connect_show(move |_| {
        if let (Some(entry), Some(calendar)) = (input.upgrade(), picker.upgrade()) {
            select_date(&calendar, &entry.text());
        }
    });
    let input = entry.downgrade();
    calendar.connect_day_selected(move |calendar| {
        if let Some(entry) = input.upgrade()
            && let Ok(value) = calendar.date().format("%Y-%m-%d")
        {
            entry.set_text(&value);
        }
    });
    let input = entry.downgrade();
    let picker = popup.downgrade();
    connect_calendar_pick(&calendar, move |date| {
        if let Some(entry) = input.upgrade().filter(|entry| entry.text() != date) {
            entry.set_text(date);
        }
        if let Some(popup) = picker.upgrade() {
            popup.popdown();
        }
    });
    (pair(&entry, &button), entry)
}

fn accessible(widget: &impl IsA<gtk::Widget>, label: &str) {
    widget.as_ref().update_property(&[gtk::accessible::Property::Label(label)]);
}

fn field(grid: &gtk::Grid, row: i32, label: &str, input: &impl IsA<gtk::Widget>) -> gtk::Label {
    let title = gtk::Label::builder().label(label).xalign(0.0).build();
    title.set_mnemonic_widget(Some(input));
    grid.attach(&title, 0, row, 1, 1);
    grid.attach(input, 1, row, 1, 1);
    title
}

fn grid() -> gtk::Grid {
    gtk::Grid::builder().column_spacing(12).row_spacing(6).build()
}

fn pair(first: &impl IsA<gtk::Widget>, second: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(first);
    row.append(second);
    row
}

fn spin(min: f64, max: f64, value: f64) -> gtk::SpinButton {
    let field = gtk::SpinButton::with_range(min, max, 1.0);
    field.set_value(value);
    field.set_numeric(true);
    field
}

fn number(field: &gtk::SpinButton) -> Result<u32> {
    field.value().to_string().parse().map_err(|_| Error::Space("Enter a whole number".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recurrence_editor_preserves_saved_patterns_when_editing_offsets() {
        if adw::init().is_err() {
            return;
        }
        let rule = Rule {
            mode: Mode::Jump,
            start: "2026-01-01".parse().unwrap(),
            pattern: "rrule:FREQ=YEARLY;INTERVAL=2;BYMONTH=11;BYDAY=-1FR,1MO;COUNT=5;WKST=SU"
                .into(),
            scheduled: Some(0),
            due: None,
        };
        let form = RuleForm::new(&rule, true).unwrap();
        let dialog = adw::AlertDialog::new(None, None);
        dialog.add_response("save", "Save");
        form.watch(&dialog);
        assert_eq!(form.read().unwrap(), rule);
        assert!(form.scheduled.enabled.is_active());
        assert!(form.scheduled.days.is_sensitive());
        assert!(!form.due.enabled.is_active());
        assert!(!form.due.days.is_sensitive());
        form.due.enabled.set_active(true);
        assert!(form.due.days.is_sensitive());
        form.due.days.set_value(3.0);
        assert_eq!(form.read().unwrap(), Rule { due: Some(3), ..rule.clone() });
        form.scheduled.enabled.set_active(false);
        assert!(!form.scheduled.days.is_sensitive());
        assert_eq!(form.read().unwrap(), Rule { due: Some(3), scheduled: None, ..rule });
    }

    #[test]
    fn recurrence_editor_shows_only_options_for_each_frequency() {
        if adw::init().is_err() {
            return;
        }
        let rule = Rule {
            mode: Mode::Keep,
            start: "2026-11-21".parse().unwrap(),
            pattern: "FREQ=WEEKLY;INTERVAL=2;COUNT=3".into(),
            scheduled: Some(0),
            due: None,
        };
        let form = RuleForm::new(&rule, false).unwrap();
        let dialog = adw::AlertDialog::new(None, None);
        dialog.add_response("save", "Save");
        form.watch(&dialog);
        assert!(form.calendar.weekly_row.is_visible());
        assert!(!form.calendar.month.is_visible());
        assert!(!form.calendar.monthly_pattern.is_visible());

        // The start is Saturday. Sunday belongs to the next week.
        form.calendar.weekdays[0].set_active(true);
        form.calendar.weekdays[6].set_active(true);
        let dates = form.read().unwrap().preview().unwrap();
        assert_eq!(
            dates,
            ["2026-11-21", "2026-11-29", "2026-12-05"].map(|text| text.parse().unwrap()),
        );

        form.frequency.set_selected(2);
        assert!(!form.calendar.weekly_row.is_visible());
        assert!(!form.calendar.month.is_visible());
        assert!(form.calendar.monthly_pattern.is_visible());
        form.calendar.monthly_pattern.set_selected(2);
        assert!(form.calendar.ordinal_row.is_visible());
        let monthly = form.read().unwrap();
        assert_eq!(monthly.pattern, "FREQ=MONTHLY;INTERVAL=2;BYDAY=1SA;COUNT=3");

        form.frequency.set_selected(3);
        assert!(form.calendar.month.is_visible());
        assert!(!form.calendar.weekly_row.is_visible());
        form.frequency.set_selected(0);
        assert!(!form.calendar.grid.is_visible());
    }

    #[test]
    fn recurrence_editor_preserves_the_saved_week_boundary_on_offset_edits() {
        if adw::init().is_err() {
            return;
        }
        let rule = Rule {
            mode: Mode::Keep,
            start: "2026-11-21".parse().unwrap(),
            pattern: "FREQ=WEEKLY;INTERVAL=2;BYDAY=SA,SU".into(),
            scheduled: Some(0),
            due: None,
        };
        let form = RuleForm::new(&rule, true).unwrap();
        form.scheduled.days.set_value(-1.0);
        assert_eq!(form.read().unwrap(), Rule { scheduled: Some(-1), ..rule });
    }
}
