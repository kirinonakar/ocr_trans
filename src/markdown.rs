use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use slint::{ComponentHandle, ModelRc, StyledText, VecModel};
use std::rc::Rc;
use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

use crate::{MarkdownBlock, MarkdownCell, TextboxWindow};

pub(crate) fn initialize(window: &TextboxWindow) {
    window.on_render_markdown(|source| ModelRc::from(Rc::new(VecModel::from(render(&source)))));
    let weak = window.as_weak();
    window.on_copy_original(move || {
        if let Some(window) = weak.upgrade() {
            if let Err(error) = crate::capture::copy_text_to_clipboard(&window.get_text()) {
                log::warn!("Failed to copy original result: {error:#}");
            }
        }
    });
}

#[derive(Default)]
struct Inline {
    markdown: String,
    plain: String,
    styles: Vec<(&'static str, &'static str)>,
}

impl Inline {
    fn text(&mut self, text: &str) {
        self.plain.push_str(text);
        for character in text.chars() {
            // Escape literal OCR/API content before passing it to the inline renderer.
            if character.is_ascii_punctuation() {
                self.markdown.push('\\');
            }
            self.markdown.push(character);
        }
    }

    fn cell(&mut self, code: bool) -> MarkdownCell {
        let inline = std::mem::take(self);
        let content = if code {
            StyledText::from_plain_text(&inline.plain)
        } else {
            StyledText::from_markdown(&inline.markdown)
                .unwrap_or_else(|_| StyledText::from_plain_text(&inline.plain))
        };
        MarkdownCell { content }
    }

    fn start_style(&mut self, open: &'static str, close: &'static str) {
        self.markdown.push_str(open);
        self.styles.push((open, close));
    }

    fn end_style(&mut self) {
        if let Some((_, close)) = self.styles.pop() {
            self.markdown.push_str(close);
        }
    }

    fn line_break(&mut self) {
        for (_, close) in self.styles.iter().rev() {
            self.markdown.push_str(close);
        }
        self.markdown.push('\n');
        self.plain.push('\n');
        for (open, _) in &self.styles {
            self.markdown.push_str(open);
        }
    }
}

fn block() -> MarkdownBlock {
    MarkdownBlock {
        font_scale: 1.0,
        ..Default::default()
    }
}

fn model(cells: Vec<MarkdownCell>) -> ModelRc<MarkdownCell> {
    ModelRc::from(Rc::new(VecModel::from(cells)))
}

/// CommonMark rejects closing emphasis between punctuation and a CJK suffix, e.g.
/// `**"사자"**는`. A zero-width WORD JOINER on the *inside* of that delimiter makes
/// its flanking rule unambiguous without adding a visible space. This is display-only.
/// Code, raw HTML, link destinations, and escaped asterisks retain their literal meaning.
fn cjk_emphasis(source: &str) -> String {
    let mut protected = vec![false; source.len()];
    for (event, range) in Parser::new(source).into_offset_iter() {
        if matches!(
            event,
            Event::Code(_)
                | Event::Html(_)
                | Event::InlineHtml(_)
                | Event::Start(Tag::CodeBlock(_) | Tag::Link { .. } | Tag::Image { .. })
        ) {
            protected[range].fill(true);
        }
    }

    fn cjk(c: char) -> bool {
        matches!(c as u32, 0x1100..=0x11ff | 0x3040..=0x30ff | 0x3130..=0x318f
            | 0x31f0..=0x31ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff
            | 0xa960..=0xa97f | 0xac00..=0xd7ff | 0xf900..=0xfaff
            | 0xff66..=0xff9f | 0x20000..=0x323af)
    }
    fn punctuation(c: char) -> bool {
        c.is_ascii_punctuation()
            || matches!(
                c.general_category_group(),
                GeneralCategoryGroup::Punctuation | GeneralCategoryGroup::Symbol
            )
    }

    let mut output = String::with_capacity(source.len());
    let mut index = 0;
    while index < source.len() {
        let character = source[index..].chars().next().unwrap();
        if character != '*' {
            output.push(character);
            index += character.len_utf8();
            continue;
        }
        let end = index + source[index..].bytes().take_while(|b| *b == b'*').count();
        let escaped = source[..index]
            .bytes()
            .rev()
            .take_while(|b| *b == b'\\')
            .count()
            % 2
            == 1;
        let neighbors = source[..index]
            .chars()
            .next_back()
            .zip(source[end..].chars().next());
        let can_fix = !escaped && !protected[index..end].iter().any(|p| *p);
        if can_fix && neighbors.is_some_and(|(before, after)| punctuation(before) && cjk(after)) {
            output.push('\u{2060}');
        }
        output.push_str(&source[index..end]);
        if can_fix && neighbors.is_some_and(|(before, after)| cjk(before) && punctuation(after)) {
            output.push('\u{2060}');
        }
        index = end;
    }
    output
}

/// Parse blocks separately because Slint's inline renderer does not support headings,
/// fenced code, tables or quotes. The source string is never changed or used for copying.
fn render(source: &str) -> Vec<MarkdownBlock> {
    let display_source = cjk_emphasis(source);
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    let mut blocks = Vec::new();
    let mut current = block();
    let mut inline = Inline::default();
    let mut cells = Vec::new();
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut quote_depth = 0;
    let mut in_table = false;

    fn flush(blocks: &mut Vec<MarkdownBlock>, current: &mut MarkdownBlock, inline: &mut Inline) {
        if !inline.plain.is_empty() || current.code {
            current.cells = model(vec![inline.cell(current.code)]);
            blocks.push(current.clone());
        }
        // Keep the container context for continuation paragraphs and nested blocks.
        current.prefix = "".into();
        current.font_scale = 1.0;
        current.heading = false;
        current.code = false;
    }

    for event in Parser::new_ext(&display_source, options) {
        match event {
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => flush(&mut blocks, &mut current, &mut inline),
            Event::Start(Tag::Heading { level, .. }) => {
                current.heading = true;
                current.font_scale = match level as u8 {
                    1 => 1.6,
                    2 => 1.4,
                    3 => 1.2,
                    _ => 1.1,
                };
            }
            Event::End(TagEnd::Heading(_)) => flush(&mut blocks, &mut current, &mut inline),
            Event::Start(Tag::CodeBlock(_)) => {
                flush(&mut blocks, &mut current, &mut inline);
                current.code = true;
            }
            Event::End(TagEnd::CodeBlock) => flush(&mut blocks, &mut current, &mut inline),
            Event::Start(Tag::BlockQuote(_)) => {
                flush(&mut blocks, &mut current, &mut inline);
                quote_depth += 1;
                current.quote = true;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(&mut blocks, &mut current, &mut inline);
                quote_depth -= 1;
                current.quote = quote_depth > 0;
            }
            Event::Start(Tag::List(start)) => {
                flush(&mut blocks, &mut current, &mut inline);
                lists.push(start);
                current.indent = lists.len().saturating_sub(1) as i32;
            }
            Event::End(TagEnd::List(_)) => {
                flush(&mut blocks, &mut current, &mut inline);
                lists.pop();
                current.indent = lists.len().saturating_sub(1) as i32;
            }
            Event::Start(Tag::Item) => {
                if let Some(Some(number)) = lists.last_mut() {
                    current.prefix = format!("{number}.").into();
                    *number = number.saturating_add(1);
                } else {
                    current.prefix = "•".into();
                }
            }
            Event::End(TagEnd::Item) => flush(&mut blocks, &mut current, &mut inline),
            Event::Start(Tag::Table(_)) => {
                flush(&mut blocks, &mut current, &mut inline);
                in_table = true;
                current.table_row = true;
            }
            Event::Start(Tag::TableHead) => current.heading = true,
            Event::End(TagEnd::TableCell) => cells.push(inline.cell(false)),
            Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
                current.cells = model(std::mem::take(&mut cells));
                blocks.push(current.clone());
                current.heading = false;
            }
            Event::End(TagEnd::Table) => {
                in_table = false;
                current.table_row = false;
            }
            Event::Rule => {
                flush(&mut blocks, &mut current, &mut inline);
                let mut rule = block();
                rule.rule = true;
                blocks.push(rule);
            }
            Event::Start(Tag::Strong) => inline.start_style("**", "**"),
            Event::Start(Tag::Emphasis) => inline.start_style("*", "*"),
            Event::Start(Tag::Strikethrough) => inline.start_style("~~", "~~"),
            Event::End(
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link,
            ) => inline.end_style(),
            // Render link labels with underline; URLs remain available in the original copy.
            Event::Start(Tag::Link { .. }) => inline.start_style("<u>", "</u>"),
            Event::Code(text) => {
                inline.plain.push_str(&text);
                let fence =
                    "`".repeat(text.split(|c| c != '`').map(str::len).max().unwrap_or(0) + 1);
                inline.markdown.push_str(&format!("{fence} {text} {fence}"));
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => inline.text(&text),
            Event::SoftBreak | Event::HardBreak => {
                // Keep line breaks while closing/reopening inline styles so spans stay valid.
                inline.line_break();
            }
            Event::TaskListMarker(checked) => inline.text(if checked { "☑ " } else { "☐ " }),
            _ => {}
        }
    }
    if !in_table {
        flush(&mut blocks, &mut current, &mut inline);
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;

    fn content(block: &MarkdownBlock, index: usize) -> StyledText {
        block.cells.row_data(index).unwrap().content
    }

    #[test]
    fn headings_and_inline_styles_render() {
        let source = "# 제목\n\n**굵게** 그리고 *기울임* ~~취소~~ `code`";
        let blocks = render(source);
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].heading);
        assert_eq!(blocks[0].font_scale, 1.6);
        assert_eq!(
            content(&blocks[0], 0),
            StyledText::from_markdown("제목").unwrap()
        );
        assert_eq!(
            content(&blocks[1], 0),
            StyledText::from_markdown("**굵게** 그리고 *기울임* ~~취소~~ `code`").unwrap()
        );
    }

    #[test]
    fn code_preserves_indentation_and_literal_markers() {
        let blocks = render("```rust\n  **literal** <tag>\n    日本語\n```\n\nnext");
        assert!(blocks[0].code);
        assert_eq!(
            content(&blocks[0], 0),
            StyledText::from_plain_text("  **literal** <tag>\n    日本語\n")
        );
        assert!(!blocks[1].code);
    }

    #[test]
    fn nested_lists_quotes_tasks_and_tables_keep_their_structure() {
        let blocks = render("3. first\n   - nested\n4. second\n\n> quote\n\n- [x] done\n\n| A | B |\n| - | - |\n| **one** | two |\n\n---");
        assert_eq!(blocks[0].prefix, "3.");
        assert_eq!(blocks[1].indent, 1);
        assert_eq!(blocks[2].prefix, "4.");
        assert!(blocks[3].quote);
        assert!(!blocks[4].quote);
        assert_eq!(
            content(&blocks[4], 0),
            StyledText::from_markdown("☑ done").unwrap()
        );
        assert!(blocks[5].table_row && blocks[5].heading);
        assert_eq!(blocks[5].cells.row_count(), 2);
        assert_eq!(
            content(&blocks[6], 0),
            StyledText::from_markdown("**one**").unwrap()
        );
        assert!(blocks[7].rule);
    }

    #[test]
    fn empty_and_literal_html_are_safe() {
        assert!(render("").is_empty());
        let blocks = render("<script>alert('test')</script>");
        assert_eq!(
            content(&blocks[0], 0),
            StyledText::from_plain_text("<script>alert('test')</script>")
        );
    }

    #[test]
    fn emphasis_spanning_lines_keeps_valid_spans() {
        let blocks = render("**first\n日本語**");
        assert_eq!(
            content(&blocks[0], 0),
            StyledText::from_markdown("**first**\n**日本語**").unwrap()
        );
    }

    #[test]
    fn cjk_quotes_and_suffixes_render_as_emphasis() {
        for source in [
            "**\"사자\"**는",
            "**“사자”**는",
            "**(사자)**는",
            "이것은**\"사자\"**입니다",
            "これは**「太字」**です",
            "这是**“粗体”**文字",
            "*\"사자\"*는",
            "***\"사자\"***는",
        ] {
            let normalized = cjk_emphasis(source);
            let mut html = String::new();
            pulldown_cmark::html::push_html(&mut html, Parser::new(&normalized));
            assert!(
                !html.contains('*'),
                "emphasis was left literal: {source}: {html}"
            );
            assert!(
                html.contains("<strong>") || html.contains("<em>"),
                "{source}"
            );
            assert_eq!(normalized.replace('\u{2060}', ""), source);
            assert_eq!(
                content(&render(source)[0], 0),
                StyledText::from_markdown(&normalized).unwrap()
            );
        }
    }

    #[test]
    fn cjk_fix_leaves_code_escapes_and_normal_markdown_alone() {
        for source in [
            "**한글**은",
            "**bold** text",
            "a**\"word\"**b",
            "`**\"사자\"**는`",
            "```\n**\"사자\"**는\n```",
            "    **\"사자\"**는",
            r#"\*\*"사자"\*\*는"#,
            "[link](https://example.com/**사자**)",
        ] {
            assert_eq!(cjk_emphasis(source), source);
        }
    }

    #[test]
    fn result_window_renders_headlessly_and_retains_original() {
        use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};

        struct Headless(Rc<MinimalSoftwareWindow>);
        impl slint::platform::Platform for Headless {
            fn create_window_adapter(
                &self,
            ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }

        let surface = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(Headless(surface.clone()))).unwrap();
        let window = TextboxWindow::new().unwrap();
        initialize(&window);
        let source = "# 마크다운 결과\n\n**\"사자\"**는 굵게 표시됩니다. 일반 글자와 **볼드체**를 비교합니다.\n\n- 첫 번째 항목\n- *기울임*과 ~~취소선~~\n\n> 인용문입니다.\n\n```rust\n  let text = \"**원문 유지**\";\n```\n\n| 항목 | 결과 |\n| --- | --- |\n| 한글 | **정상** |\n\n긴 문장은 결과창 폭에 맞춰 자동으로 줄바꿈되어야 합니다. 긴 문장은 결과창 폭에 맞춰 자동으로 줄바꿈되어야 합니다.";
        window.set_text(source.into());
        window.show().unwrap();
        for (name, width, height, dark) in [
            ("light", 600, 580, false),
            ("dark", 600, 580, true),
            ("narrow", 320, 580, false),
        ] {
            window.set_dark_theme(dark);
            surface.set_size(slint::PhysicalSize::new(width, height));
            window.window().request_redraw();
            let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
            assert!(surface.draw_if_needed(|renderer| {
                renderer.render(&mut pixels, width as usize);
            }));
            assert!(pixels.iter().any(|pixel| *pixel != pixels[0]));
            let bytes: Vec<u8> = pixels
                .iter()
                .flat_map(|pixel| [pixel.r, pixel.g, pixel.b])
                .collect();
            let directory =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/markdown-preview");
            std::fs::create_dir_all(&directory).unwrap();
            image::save_buffer(
                directory.join(format!("{name}.png")),
                &bytes,
                width,
                height,
                image::ColorType::Rgb8,
            )
            .unwrap();
            assert_eq!(window.get_text(), source);
        }
        window.set_text("".into());
        assert_eq!(window.get_text(), "");
        window.hide().unwrap();
    }
}
