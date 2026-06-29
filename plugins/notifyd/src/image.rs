use std::collections::BTreeMap;

use zbus::zvariant::OwnedValue;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct NormalizedImage {
    pub icon_name: String,
    pub image_path: Option<String>,
    pub image_source: String,
    pub image_width: Option<u32>,
    pub image_height: Option<u32>,
}

pub(crate) fn normalize_image(
    app_icon: &str,
    hints: &BTreeMap<String, OwnedValue>,
    body_images: bool,
) -> NormalizedImage {
    if body_images {
        if let Some(path) =
            hint_string(hints, "image-path").or_else(|| hint_string(hints, "image_path"))
        {
            if let Some(path) = normalize_file_path(&path) {
                return NormalizedImage {
                    icon_name: fallback_icon(app_icon),
                    image_path: Some(path),
                    image_source: "image-path".to_owned(),
                    image_width: None,
                    image_height: None,
                };
            }
        }

        if hints.contains_key("image-data")
            || hints.contains_key("image_data")
            || hints.contains_key("icon_data")
        {
            return NormalizedImage {
                icon_name: fallback_icon(app_icon),
                image_path: None,
                image_source: "image-data".to_owned(),
                image_width: None,
                image_height: None,
            };
        }
    }

    if let Some(path) = normalize_file_path(app_icon) {
        return NormalizedImage {
            icon_name: String::new(),
            image_path: Some(path),
            image_source: "app-icon".to_owned(),
            image_width: None,
            image_height: None,
        };
    }

    NormalizedImage {
        icon_name: fallback_icon(app_icon),
        image_path: None,
        image_source: if app_icon.trim().is_empty() {
            "none".to_owned()
        } else {
            "icon-name".to_owned()
        },
        image_width: None,
        image_height: None,
    }
}

fn normalize_file_path(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value.strip_prefix("file://").unwrap_or(value);
    value.starts_with('/').then(|| value.to_owned())
}

fn fallback_icon(app_icon: &str) -> String {
    let app_icon = app_icon.trim();
    if app_icon.is_empty() || app_icon.starts_with('/') || app_icon.starts_with("file://") {
        "dialog-information-symbolic".to_owned()
    } else {
        app_icon.to_owned()
    }
}

fn hint_string(values: &BTreeMap<String, OwnedValue>, key: &str) -> Option<String> {
    values
        .get(key)
        .and_then(|value| String::try_from(value.to_owned()).ok())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use zbus::zvariant::{OwnedValue, Value};

    use super::normalize_image;

    #[test]
    fn prefers_image_path_hint() {
        let hints = BTreeMap::from([(
            "image-path".to_owned(),
            OwnedValue::try_from(Value::from("/tmp/test.png")).unwrap(),
        )]);

        let image = normalize_image("dialog-information", &hints, true);

        assert_eq!(image.image_path.as_deref(), Some("/tmp/test.png"));
        assert_eq!(image.image_source, "image-path");
    }
}
