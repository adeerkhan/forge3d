//! The schema-driven terrain parameter pages (GUI.md G4).
//!
//! `terrain.params_schema` introspects `make_terrain_params_config` in the
//! engine: every root parameter, every nested settings section, the enum
//! choices the engine accepts and the defaults a render really uses. These
//! pages render that schema, so a new engine parameter appears without a Rust
//! change and a parameter the pages cannot edit says why.
//!
//! Validation is the engine's: an edit updates the project, schedules a
//! debounced `terrain.params_validate`, and the engine's own message is shown
//! inline. Only accepted parameters can reach a render.

use std::collections::HashSet;

use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme as _, IconName, Sizable as _, StyledExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::state::Studio;

/// How a schema field is edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    Int,
    Float,
    Str,
    FloatArray,
    IntArray,
}

impl ParamKind {
    pub fn from_schema(kind: &str) -> Option<Self> {
        match kind {
            "int" => Some(ParamKind::Int),
            "float" => Some(ParamKind::Float),
            "str" => Some(ParamKind::Str),
            "float[]" => Some(ParamKind::FloatArray),
            "int[]" => Some(ParamKind::IntArray),
            _ => None,
        }
    }

    /// The canonical text a committed value shows in its input.
    pub fn render(self, value: &serde_json::Value) -> String {
        match self {
            ParamKind::FloatArray | ParamKind::IntArray => value
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            item.as_f64()
                                // 6000.0 reads better as "6000" in an editable box.
                                .map(|number| number.to_string())
                                .unwrap_or_else(|| item.to_string())
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default(),
            _ => value
                .as_f64()
                .map(|number| number.to_string())
                .or_else(|| value.as_str().map(str::to_owned))
                .unwrap_or_default(),
        }
    }

    /// Parse a text box. `None` means "not a complete value yet" — the draft is
    /// kept, so a half-typed `1.` or `1, 2,` is never fought across frames.
    pub fn parse(self, text: &str, nullable: bool) -> Option<serde_json::Value> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return nullable.then_some(serde_json::Value::Null);
        }
        match self {
            ParamKind::Int => trimmed
                .parse::<i64>()
                .ok()
                .map(|value| serde_json::json!(value)),
            ParamKind::Float => trimmed
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .map(|value| serde_json::json!(value)),
            ParamKind::Str => Some(serde_json::json!(trimmed)),
            ParamKind::FloatArray => {
                let mut values = Vec::new();
                for part in trimmed.split(',') {
                    let value = part.trim().parse::<f64>().ok()?;
                    if !value.is_finite() {
                        return None;
                    }
                    values.push(value);
                }
                Some(serde_json::json!(values))
            }
            ParamKind::IntArray => {
                let mut values = Vec::new();
                for part in trimmed.split(',') {
                    values.push(part.trim().parse::<i64>().ok()?);
                }
                Some(serde_json::json!(values))
            }
        }
    }
}

/// Per-session editing state for the pages: which sections are open, the live
/// text inputs, and the debounce for engine validation.
#[derive(Default)]
pub struct ParamForm {
    pub expanded: HashSet<String>,
    pub excluded_open: bool,
    pub inputs: std::collections::HashMap<String, Entity<InputState>>,
    pub kinds: std::collections::HashMap<String, (ParamKind, bool)>,
    pub validate_due: Option<std::time::Instant>,
    /// Bumped when something outside the pages changed the parameters
    /// (project load, new project), so the inputs re-read them once.
    pub revision: u64,
    synced_revision: u64,
}

fn title_case(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for (index, word) in key.split('_').enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// The page tree. Reads the schema and the project; every edit goes back
/// through [`Studio`].
pub fn params_pages(studio: &Entity<Studio>, window: &mut Window, cx: &mut App) -> AnyElement {
    let (schema, params_error, expanded, excluded_open) = {
        let state = studio.read(cx);
        (
            state.params_schema.clone(),
            state.params_error.clone(),
            state.params_form.expanded.clone(),
            state.params_form.excluded_open,
        )
    };
    let Some(schema) = schema else {
        return div()
            .p_2()
            .text_xs()
            .text_color(cx.theme().colors.muted_foreground)
            .child("Reading the engine's parameter schema…")
            .into_any_element();
    };

    let mut body = v_flex().w_full().gap_2();
    body = body.child(
        div()
            .text_xs()
            .font_semibold()
            .text_color(cx.theme().colors.muted_foreground)
            .child("Terrain parameters"),
    );

    if let Some(message) = params_error {
        body = body.child(
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().colors.danger)
                        .child(format!("⚠ {message}")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(
                            "The renderer keeps the last accepted parameters until this is fixed.",
                        ),
                ),
        );
    }

    let sections = schema
        .get("sections")
        .and_then(|sections| sections.as_array())
        .cloned()
        .unwrap_or_default();

    for section in sections {
        let Some(section_key) = section.get("key").and_then(|key| key.as_str()) else {
            continue;
        };
        let editable = section
            .get("editable")
            .and_then(|editable| editable.as_bool())
            .unwrap_or(false);
        let fields = section
            .get("fields")
            .and_then(|fields| fields.as_array())
            .cloned()
            .unwrap_or_default();

        if section_key == "root" {
            for field in &fields {
                body = body.child(field_row(studio, "root", field, window, cx));
            }
            continue;
        }

        let open = expanded.contains(section_key);
        let studio_toggle = studio.clone();
        let key_owned = section_key.to_owned();
        body = body.child(
            v_flex().w_full().gap_1().child(
                h_flex()
                    .id(format!("params-section-{section_key}"))
                    .justify_between()
                    .gap_2()
                    .cursor_pointer()
                    .py(px(2.))
                    .on_click(move |_, _, cx| {
                        let key = key_owned.clone();
                        studio_toggle.update(cx, |studio, cx| {
                            if !studio.params_form.expanded.remove(&key) {
                                studio.params_form.expanded.insert(key);
                            }
                            cx.notify();
                        });
                    })
                    .child(
                        h_flex()
                            .gap_1()
                            .child(IconName::ChevronRight)
                            .when(open, |row| row.child(""))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if editable {
                                        cx.theme().colors.foreground
                                    } else {
                                        cx.theme().colors.muted_foreground
                                    })
                                    .child(title_case(section_key)),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(format!("{}", fields.len())),
                    ),
            ),
        );

        if open {
            if !editable {
                let reason = section
                    .get("reason")
                    .and_then(|reason| reason.as_str())
                    .unwrap_or("not editable from Studio")
                    .to_owned();
                body = body.child(
                    div()
                        .pl_3()
                        .text_xs()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(reason),
                );
            } else {
                let mut fields_block = v_flex().w_full().gap_1().pl_3();
                for field in &fields {
                    fields_block =
                        fields_block.child(field_row(studio, section_key, field, window, cx));
                }
                body = body.child(fields_block);
            }
        }
    }

    // Honest reporting: parameters the pages cannot edit, and why.
    if let Some(excluded) = schema.get("excluded").and_then(|list| list.as_array()) {
        if !excluded.is_empty() {
            let open = excluded_open;
            let studio_toggle = studio.clone();
            body = body.child(
                h_flex()
                    .id("params-excluded")
                    .gap_1()
                    .cursor_pointer()
                    .py(px(2.))
                    .on_click(move |_, _, cx| {
                        studio_toggle.update(cx, |studio, cx| {
                            studio.params_form.excluded_open = !studio.params_form.excluded_open;
                            cx.notify();
                        });
                    })
                    .child(IconName::ChevronRight)
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(format!("Not exposed ({})", excluded.len())),
                    ),
            );
            if open {
                let mut list = v_flex().w_full().gap_1().pl_3();
                for entry in excluded {
                    let key = entry.get("key").and_then(|key| key.as_str()).unwrap_or("?");
                    let reason = entry
                        .get("reason")
                        .and_then(|reason| reason.as_str())
                        .unwrap_or("engine-internal");
                    list = list.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(format!("{key} — {reason}")),
                    );
                }
                body = body.child(list);
            }
        }
    }

    body.into_any_element()
}

/// One schema field, rendered by its type.
fn field_row(
    studio: &Entity<Studio>,
    section: &str,
    field: &serde_json::Value,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some(key) = field.get("key").and_then(|key| key.as_str()) else {
        return div().into_any_element();
    };
    let Some(kind_name) = field.get("type").and_then(|kind| kind.as_str()) else {
        return div().into_any_element();
    };
    let nullable = field
        .get("nullable")
        .and_then(|nullable| nullable.as_bool())
        .unwrap_or(false);
    let default = field
        .get("default")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let choices: Vec<String> = field
        .get("choices")
        .and_then(|choices| choices.as_array())
        .map(|choices| {
            choices
                .iter()
                .filter_map(|choice| choice.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    let current = studio.read(cx).param_value(section, key, &default);
    let label = title_case(key);

    // Booleans are a checkbox.
    if kind_name == "bool" {
        let checked = current.as_bool().unwrap_or(false);
        let section_owned = section.to_owned();
        let key_owned = key.to_owned();
        let studio = studio.clone();
        return h_flex()
            .justify_between()
            .gap_2()
            .child(div().text_xs().child(label))
            .child(
                Checkbox::new(format!("param-{section}-{key}"))
                    .checked(checked)
                    .on_click(move |checked, _, cx| {
                        let value = serde_json::json!(*checked);
                        let (section, key) = (section_owned.clone(), key_owned.clone());
                        studio.update(cx, |studio, cx| studio.set_param(&section, &key, value, cx));
                    }),
            )
            .into_any_element();
    }

    // Enumerations: the choices come from the engine, so a bad value can only
    // come from a free-text field (which validation catches).
    if kind_name == "str" && !choices.is_empty() {
        let selected = current.as_str().unwrap_or_default().to_owned();
        let section_owned = section.to_owned();
        let key_owned = key.to_owned();
        let studio = studio.clone();
        let studio_menu = studio.clone();
        return h_flex()
            .justify_between()
            .gap_2()
            .child(div().text_xs().child(label))
            .child(
                Button::new(format!("param-{section}-{key}"))
                    .label(selected.clone())
                    .xsmall()
                    .outline()
                    .dropdown_menu(move |menu, _, _| {
                        let mut menu = menu;
                        for choice in &choices {
                            let value = serde_json::json!(choice.clone());
                            let (section, key) = (section_owned.clone(), key_owned.clone());
                            let studio = studio_menu.clone();
                            menu = menu.item(PopupMenuItem::new(choice.clone()).on_click(
                                move |_, _, cx| {
                                    let (section, key, value) =
                                        (section.clone(), key.clone(), value.clone());
                                    studio.update(cx, |studio, cx| {
                                        studio.set_param(&section, &key, value, cx)
                                    });
                                },
                            ));
                        }
                        menu
                    }),
            )
            .into_any_element();
    }

    // Everything else is a text box parsed on every keystroke; nothing reaches
    // the project until the text is a complete value.
    let Some(kind) = ParamKind::from_schema(kind_name) else {
        return div()
            .text_xs()
            .text_color(cx.theme().colors.muted_foreground)
            .child(format!("{label} — unsupported type {kind_name}"))
            .into_any_element();
    };
    let text = kind.render(&current);
    let input = studio.update(cx, |studio, cx| {
        studio.ensure_param_input(section, key, &text, kind, nullable, window, cx)
    });

    h_flex()
        .justify_between()
        .gap_2()
        .child(div().text_xs().child(label))
        .child(Input::new(&input).small().w(px(140.)))
        .into_any_element()
}

/// Re-read the committed parameters into the live inputs, once, after
/// something outside the pages changed them (project load).
pub fn sync_inputs(studio: &mut Studio, window: &mut Window, cx: &mut Context<Studio>) {
    if studio.params_form.synced_revision == studio.params_form.revision {
        return;
    }
    studio.params_form.synced_revision = studio.params_form.revision;
    let schema = studio.params_schema.clone();
    let keys: Vec<String> = studio.params_form.inputs.keys().cloned().collect();
    for id in keys {
        let Some((section, key)) = id.split_once('.') else {
            continue;
        };
        let Some((kind, _nullable)) = studio.params_form.kinds.get(&id).copied() else {
            continue;
        };
        let default = schema
            .as_ref()
            .and_then(|schema| field_default(schema, section, key))
            .unwrap_or(serde_json::Value::Null);
        let value = studio.param_value(section, key, &default);
        let text = kind.render(&value);
        if let Some(input) = studio.params_form.inputs.get(&id).cloned() {
            input.update(cx, |state, cx| state.set_value(text, window, cx));
        }
    }
}

/// The schema default for one field, used when a field is absent from the
/// project.
pub fn field_default(
    schema: &serde_json::Value,
    section: &str,
    key: &str,
) -> Option<serde_json::Value> {
    schema
        .get("sections")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("key").and_then(|k| k.as_str()) == Some(section))?
        .get("fields")?
        .as_array()?
        .iter()
        .find(|field| field.get("key").and_then(|k| k.as_str()) == Some(key))?
        .get("default")
        .cloned()
}

/// Every field default of one section, for materializing a settings object the
/// first time one of its fields is edited.
pub fn section_defaults(schema: &serde_json::Value, section: &str) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    if let Some(fields) = schema
        .get("sections")
        .and_then(|sections| sections.as_array())
        .and_then(|sections| {
            sections
                .iter()
                .find(|entry| entry.get("key").and_then(|k| k.as_str()) == Some(section))
        })
        .and_then(|entry| entry.get("fields"))
        .and_then(|fields| fields.as_array())
    {
        for field in fields {
            if let (Some(key), Some(default)) = (
                field.get("key").and_then(|key| key.as_str()),
                field.get("default"),
            ) {
                map.insert(key.to_owned(), default.clone());
            }
        }
    }
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn parse_waits_for_complete_values() {
        assert_eq!(
            ParamKind::Float.parse("-", false),
            None,
            "a sign alone is not a value"
        );
        assert_eq!(
            ParamKind::Float.parse("1.5", false),
            Some(serde_json::json!(1.5))
        );
        assert_eq!(
            ParamKind::Float.parse("", true),
            Some(serde_json::Value::Null)
        );
        assert_eq!(ParamKind::Float.parse("", false), None);
        assert_eq!(
            ParamKind::FloatArray.parse("1, 2,", false),
            None,
            "a trailing comma is incomplete"
        );
        assert_eq!(
            ParamKind::FloatArray.parse("1, 2", false),
            Some(serde_json::json!([1.0, 2.0]))
        );
        assert_eq!(
            ParamKind::IntArray.parse("4,5", false),
            Some(serde_json::json!([4, 5]))
        );
        assert_eq!(ParamKind::Int.parse("4.5", false), None);
    }

    #[test]
    fn render_is_canonical() {
        assert_eq!(ParamKind::Float.render(&serde_json::json!(6000.0)), "6000");
        assert_eq!(
            ParamKind::FloatArray.render(&serde_json::json!([1.5, 2.0])),
            "1.5, 2"
        );
        assert_eq!(ParamKind::Str.render(&serde_json::json!("magma")), "magma");
    }

    #[test]
    fn section_titles_read_like_labels() {
        assert_eq!(title_case("water_settings"), "Water Settings");
        assert_eq!(title_case("root"), "Root");
    }
}
