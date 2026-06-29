pub(crate) fn plain_text(body: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;

    for char in body.chars() {
        match char {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => output.push(char),
            _ => {}
        }
    }

    decode_entities(output.trim())
}

pub(crate) fn sanitized_markup(body: &str, enabled: bool) -> Option<String> {
    if !enabled || !body.contains('<') {
        return None;
    }
    Some(body.trim().to_owned())
}

fn decode_entities(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use super::plain_text;

    #[test]
    fn strips_simple_markup() {
        assert_eq!(plain_text("<b>Hello</b> &amp; goodbye"), "Hello & goodbye");
    }
}
