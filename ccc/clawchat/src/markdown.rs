/// Render markdown text to safe HTML for display in messages.
/// Uses pulldown-cmark. Script tags are escaped as a basic safety measure.
pub fn render_markdown(text: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};

    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_SMART_PUNCTUATION);

    let parser = Parser::new_ext(text, opts);
    let mut html_out = String::with_capacity(text.len() * 2);
    html::push_html(&mut html_out, parser);

    // Strip script tags (basic XSS mitigation for trusted-team use)
    html_out
        .replace("<script", "&lt;script")
        .replace("</script>", "&lt;/script&gt;")
}

/// Return true if the text looks like it contains markdown.
/// Used to decide whether to run through the full renderer.
pub fn has_markdown(text: &str) -> bool {
    text.contains("**")
        || text.contains("__")
        || text.contains('`')
        || text.contains("```")
        || text.contains("~~")
        || text.contains("- [")
        || text.starts_with('#')
        || text.starts_with('>')
        || text.starts_with("- ")
        || text.contains("[http")
        || text.contains("](")
}
