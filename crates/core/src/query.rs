//! Database queries: filtering, sorting and grouping rows for a view,
//! plus small date helpers for the calendar.

use crate::model::*;
use chrono::{Datelike, NaiveDate};
use std::cmp::Ordering;

/// Text of a property for a row (the title property reads the page title).
pub fn prop_text(row: &Page, prop: &Property) -> String {
    if prop.kind == PropKind::Title {
        return row.title.clone();
    }
    row.props.get(&prop.name).map(|v| v.as_text()).unwrap_or_default()
}

fn prop_is_empty(row: &Page, prop: &Property) -> bool {
    if prop.kind == PropKind::Title {
        return row.title.is_empty();
    }
    row.props.get(&prop.name).is_none_or(|v| v.is_empty())
}

pub fn matches_filter(row: &Page, db: &Database, filter: &Filter) -> bool {
    let Some(prop) = db.prop(&filter.prop) else {
        return true;
    };
    let needle = filter.value.trim().to_lowercase();
    // An unfinished rule (no value typed yet) does not filter anything out.
    if filter.op.needs_value() && needle.is_empty() {
        return true;
    }
    let text = prop_text(row, prop).to_lowercase();
    let value = row.props.get(&prop.name);
    match filter.op {
        FilterOp::IsEmpty => prop_is_empty(row, prop),
        FilterOp::IsNotEmpty => !prop_is_empty(row, prop),
        FilterOp::Checked => value.is_some_and(|v| v.as_bool()),
        FilterOp::Unchecked => !value.is_some_and(|v| v.as_bool()),
        FilterOp::Contains => match prop.kind {
            PropKind::MultiSelect => value
                .map(|v| v.as_list())
                .unwrap_or_default()
                .iter()
                .any(|o| o.to_lowercase() == needle || o.to_lowercase().contains(&needle)),
            _ => text.contains(&needle),
        },
        FilterOp::NotContains => match prop.kind {
            PropKind::MultiSelect => !value
                .map(|v| v.as_list())
                .unwrap_or_default()
                .iter()
                .any(|o| o.to_lowercase() == needle || o.to_lowercase().contains(&needle)),
            _ => !text.contains(&needle),
        },
        FilterOp::Is => match prop.kind {
            PropKind::Number => {
                value.and_then(|v| v.as_number()) == needle.parse::<f64>().ok()
            }
            _ => text == needle,
        },
        FilterOp::IsNot => match prop.kind {
            PropKind::Number => {
                value.and_then(|v| v.as_number()) != needle.parse::<f64>().ok()
            }
            _ => text != needle,
        },
        FilterOp::Gt | FilterOp::Lt => {
            let ord = match prop.kind {
                PropKind::Number => {
                    let (Some(a), Ok(b)) =
                        (value.and_then(|v| v.as_number()), needle.parse::<f64>())
                    else {
                        return false;
                    };
                    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
                }
                _ => {
                    if text.is_empty() {
                        return false;
                    }
                    text.as_str().cmp(needle.as_str())
                }
            };
            if filter.op == FilterOp::Gt {
                ord == Ordering::Greater
            } else {
                ord == Ordering::Less
            }
        }
    }
}

fn compare_rows(a: &Page, b: &Page, prop: &Property) -> Ordering {
    let ea = prop_is_empty(a, prop) && prop.kind != PropKind::Checkbox;
    let eb = prop_is_empty(b, prop) && prop.kind != PropKind::Checkbox;
    match (ea, eb) {
        (true, true) => return Ordering::Equal,
        // empty values always sort last (handled by the caller for desc)
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    let va = a.props.get(&prop.name);
    let vb = b.props.get(&prop.name);
    match prop.kind {
        PropKind::Number => {
            let x = va.and_then(|v| v.as_number()).unwrap_or(0.0);
            let y = vb.and_then(|v| v.as_number()).unwrap_or(0.0);
            x.partial_cmp(&y).unwrap_or(Ordering::Equal)
        }
        PropKind::Checkbox => {
            let x = va.is_some_and(|v| v.as_bool());
            let y = vb.is_some_and(|v| v.as_bool());
            x.cmp(&y)
        }
        PropKind::Select | PropKind::Status => {
            let ix = |v: Option<&PropValue>| {
                let name = v.map(|v| v.as_text()).unwrap_or_default();
                prop.options
                    .iter()
                    .position(|o| o.name == name)
                    .unwrap_or(usize::MAX)
            };
            ix(va).cmp(&ix(vb))
        }
        _ => prop_text(a, prop)
            .to_lowercase()
            .cmp(&prop_text(b, prop).to_lowercase()),
    }
}

/// Rows of a view after filters and sorts. Input must be in manual order.
pub fn query<'a>(db: &Database, view: &View, rows: Vec<&'a Page>) -> Vec<&'a Page> {
    let mut rows: Vec<&Page> = rows
        .into_iter()
        .filter(|r| view.filters.iter().all(|f| matches_filter(r, db, f)))
        .collect();
    if !view.sorts.is_empty() {
        rows.sort_by(|a, b| {
            for s in &view.sorts {
                let Some(prop) = db.prop(&s.prop) else {
                    continue;
                };
                let ea = prop_is_empty(a, prop) && prop.kind != PropKind::Checkbox;
                let eb = prop_is_empty(b, prop) && prop.kind != PropKind::Checkbox;
                let mut ord = compare_rows(a, b, prop);
                if s.desc && !ea && !eb {
                    ord = ord.reverse();
                }
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            Ordering::Equal
        });
    }
    rows
}

pub struct Group<'a> {
    /// `None` is the "no value" group.
    pub option: Option<SelectOption>,
    pub rows: Vec<&'a Page>,
}

/// Group rows by a select/status/multi-select property (board view).
pub fn group<'a>(prop: &Property, rows: &[&'a Page]) -> Vec<Group<'a>> {
    let mut groups: Vec<Group> = Vec::new();
    groups.push(Group {
        option: None,
        rows: vec![],
    });
    for o in &prop.options {
        groups.push(Group {
            option: Some(o.clone()),
            rows: vec![],
        });
    }
    for row in rows {
        let values = row
            .props
            .get(&prop.name)
            .map(|v| v.as_list())
            .unwrap_or_default();
        let first = values.first();
        let ix = first
            .and_then(|name| {
                groups
                    .iter()
                    .position(|g| g.option.as_ref().is_some_and(|o| &o.name == name))
            })
            .unwrap_or(0);
        groups[ix].rows.push(row);
    }
    groups
}

// ---------------------------------------------------------------------------
// Dates
// ---------------------------------------------------------------------------

pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

pub fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim().get(..10)?, "%Y-%m-%d").ok()
}

pub fn date_to_string(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

const MONTHS: [&str; 12] = [
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
];

pub fn month_name(month: u32) -> &'static str {
    MONTHS[((month.max(1) - 1) % 12) as usize]
}

/// "November 20, 2026" (Notion's default date format).
pub fn format_date(s: &str) -> String {
    match parse_date(s) {
        Some(d) => format!("{} {}, {}", month_name(d.month()), d.day(), d.year()),
        None => s.to_string(),
    }
}

pub fn add_months(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let total = year * 12 + (month as i32 - 1) + delta;
    (total.div_euclid(12), (total.rem_euclid(12) + 1) as u32)
}

/// The days shown in a month grid: whole weeks, Sunday first.
pub fn month_grid(year: i32, month: u32) -> Vec<NaiveDate> {
    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_else(today);
    let lead = first.weekday().num_days_from_sunday() as i64;
    let start = first - chrono::Duration::days(lead);
    let (ny, nm) = add_months(year, month, 1);
    let next_first = NaiveDate::from_ymd_opt(ny, nm, 1).unwrap_or(first);
    let days_in_month = (next_first - first).num_days();
    let cells = ((lead + days_in_month + 6) / 7) * 7;
    (0..cells).map(|i| start + chrono::Duration::days(i)).collect()
}

/// "just now", "5m ago", "Nov 3" …
pub fn relative_time(iso: &str) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return String::new();
    };
    let secs = (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds();
    if secs < 60 {
        "just now".into()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else if secs < 86400 * 7 {
        format!("{}d ago", secs / 86400)
    } else {
        let local = t.with_timezone(&chrono::Local);
        format!("{} {}", &month_name(local.month())[..3], local.day())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(title: &str, status: &str, n: f64) -> Page {
        let mut p = Page::new(PageKind::Page, Some("db".into()));
        p.title = title.into();
        if !status.is_empty() {
            p.props.insert("Status".into(), PropValue::Text(status.into()));
        }
        p.props.insert("N".into(), PropValue::Number(n));
        p
    }

    #[test]
    fn filter_sort_group() {
        let mut db = Database::new_default();
        db.schema.push(Property::new("Status", PropKind::Status));
        db.schema.push(Property::new("N", PropKind::Number));
        let a = row("Alpha", "Done", 3.0);
        let b = row("beta", "In progress", 1.0);
        let c = row("Gamma", "", 2.0);
        let rows = vec![&a, &b, &c];

        let mut view = View::new("v", ViewKind::Table);
        view.sorts.push(Sort {
            prop: "N".into(),
            desc: true,
        });
        let out = query(&db, &view, rows.clone());
        assert_eq!(
            out.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            vec!["Alpha", "Gamma", "beta"]
        );

        view.sorts = vec![Sort {
            prop: "Status".into(),
            desc: false,
        }];
        let out = query(&db, &view, rows.clone());
        assert_eq!(
            out.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            vec!["beta", "Alpha", "Gamma"]
        );

        view.sorts.clear();
        view.filters.push(Filter {
            prop: "Name".into(),
            op: FilterOp::Contains,
            value: "a".into(),
        });
        view.filters.push(Filter {
            prop: "N".into(),
            op: FilterOp::Gt,
            value: "1".into(),
        });
        let out = query(&db, &view, rows.clone());
        assert_eq!(out.len(), 2);

        let groups = group(db.prop("Status").unwrap(), &rows);
        assert_eq!(groups.len(), 4);
        assert_eq!(groups[0].rows.len(), 1);
        assert_eq!(groups[3].rows[0].title, "Alpha");
    }

    #[test]
    fn dates() {
        assert_eq!(format_date("2026-11-20"), "November 20, 2026");
        let grid = month_grid(2026, 11);
        assert_eq!(grid.len(), 35);
        assert_eq!(date_to_string(grid[0]), "2026-11-01");
        assert_eq!(add_months(2026, 12, 1), (2027, 1));
        assert_eq!(add_months(2026, 1, -1), (2025, 12));
        assert_eq!(month_grid(2026, 8).len(), 42);
    }
}
