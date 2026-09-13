use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use slint::{ComponentHandle, ModelRc, StyledText, VecModel};
use std::rc::Rc;
use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

use crate::{MarkdownBlock, MarkdownCell, MarkdownRow, TextboxWindow};

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

    fn cell(&mut self, code: bool, heading: bool) -> MarkdownCell {
        let inline = std::mem::take(self);
        let content = if code {
            StyledText::from_plain_text(&inline.plain)
        } else {
            // Bold header cells through the same inline markdown pipeline: the
            // escaped source is wrapped as-is, so stray markup stays literal.
            let markdown = if heading && !inline.markdown.trim().is_empty() {
                format!("**{}**", inline.markdown.trim())
            } else {
                inline.markdown
            };
            StyledText::from_markdown(&markdown)
                .unwrap_or_else(|_| StyledText::from_plain_text(&inline.plain))
        };
        MarkdownCell { content, heading }
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

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(items)))
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

/// Arrow commands from LaTeX math (`$\leftarrow$` and friends) mapped to Unicode arrows.
/// OCR and translation results often contain them, but the textbox only understands inline
/// markdown, so they would otherwise be shown verbatim.
fn latex_arrow(name: &str) -> Option<&'static str> {
    Some(match name {
        "leftarrow" | "gets" => "←",
        "rightarrow" | "to" => "→",
        "leftrightarrow" => "↔",
        "Leftarrow" => "⇐",
        "Rightarrow" => "⇒",
        "Leftrightarrow" => "⇔",
        "Longleftarrow" => "⟸",
        "Longrightarrow" | "implies" => "⟹",
        "Longleftrightarrow" | "iff" => "⟺",
        "uparrow" => "↑",
        "downarrow" => "↓",
        "updownarrow" => "↕",
        "Uparrow" => "⇑",
        "Downarrow" => "⇓",
        "Updownarrow" => "⇕",
        "mapsto" => "↦",
        "longmapsto" => "⟼",
        "longleftarrow" => "⟵",
        "longrightarrow" => "⟶",
        "longleftrightarrow" => "⟷",
        "nearrow" => "↗",
        "searrow" => "↘",
        "swarrow" => "↙",
        "nwarrow" => "↖",
        "hookleftarrow" => "↩",
        "hookrightarrow" => "↪",
        "leftarrowtail" => "↢",
        "rightarrowtail" => "↣",
        "twoheadleftarrow" => "↞",
        "twoheadrightarrow" => "↠",
        "leftleftarrows" => "⇇",
        "rightrightarrows" => "⇉",
        "upuparrows" => "⇈",
        "downdownarrows" => "⇊",
        "leftrightarrows" => "⇆",
        "rightleftarrows" => "⇄",
        "leftrightharpoons" => "⇋",
        "rightleftharpoons" => "⇌",
        "leftharpoonup" => "↼",
        "leftharpoondown" => "↽",
        "rightharpoonup" => "⇀",
        "rightharpoondown" => "⇁",
        "upharpoonleft" => "↿",
        "upharpoonright" => "↾",
        "downharpoonleft" => "⇃",
        "downharpoonright" => "⇂",
        "rightsquigarrow" | "leadsto" => "↝",
        "leftrightsquigarrow" => "↭",
        "curvearrowleft" => "↶",
        "curvearrowright" => "↷",
        "circlearrowleft" => "↺",
        "circlearrowright" => "↻",
        "dashleftarrow" => "⇠",
        "dashrightarrow" => "⇢",
        "Lleftarrow" => "⇚",
        "Rrightarrow" => "⇛",
        "Lsh" => "↰",
        "Rsh" => "↱",
        "looparrowleft" => "↫",
        "looparrowright" => "↬",
        "nleftarrow" => "↚",
        "nrightarrow" => "↛",
        "nleftrightarrow" => "↮",
        "nLeftarrow" => "⇍",
        "nRightarrow" => "⇏",
        "nLeftrightarrow" => "⇎",
        _ => return None,
    })
}

/// Replace every known `\command` arrow with its Unicode symbol.
fn replace_latex_arrows(text: &str) -> String {
    if !text.contains('\\') {
        return text.to_string();
    }
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let character = text[index..].chars().next().unwrap();
        if character != '\\' {
            output.push(character);
            index += character.len_utf8();
            continue;
        }
        let start = index + 1;
        let end = start + text[start..].bytes().take_while(u8::is_ascii_alphabetic).count();
        match latex_arrow(&text[start..end]) {
            Some(symbol) => {
                output.push_str(symbol);
                index = end;
            }
            None => {
                output.push('\\');
                index += 1;
            }
        }
    }
    output
}

/// Display-only pass: render LaTeX arrows as Unicode symbols and drop the `$` delimiters of
/// the math spans that contain them, so `$\leftarrow$` becomes `←`. Code spans, code blocks,
/// links, unknown commands and currency keep their literal text; copying uses the source.
fn latex_arrows(source: &str) -> String {
    if !source.contains('\\') {
        return source.to_string();
    }
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

    let mut output = String::with_capacity(source.len());
    let mut index = 0;
    while index < source.len() {
        let character = source[index..].chars().next().unwrap();
        if protected[index] {
            output.push(character);
            index += character.len_utf8();
            continue;
        }
        match character {
            '$' => {
                // Pair the delimiters on the same line so stray currency dollars stay intact.
                let line_end = source[index + 1..]
                    .find('\n')
                    .map_or(source.len(), |offset| index + 1 + offset);
                let closer = source[index + 1..line_end]
                    .match_indices('$')
                    .map(|(offset, _)| index + 1 + offset)
                    .find(|position| !protected[*position]);
                match closer {
                    // `$$` opens nothing: keep the first dollar and rescan from the next one.
                    Some(end) if end == index + 1 => {
                        output.push('$');
                        index += 1;
                    }
                    Some(end) => {
                        let inner = &source[index + 1..end];
                        let converted = replace_latex_arrows(inner);
                        if converted == inner {
                            output.push_str(&source[index..=end]);
                        } else {
                            output.push_str(&converted);
                        }
                        index = end + 1;
                    }
                    None => {
                        output.push('$');
                        index += 1;
                    }
                }
            }
            '\\' => {
                let start = index + 1;
                let end =
                    start + source[start..].bytes().take_while(u8::is_ascii_alphabetic).count();
                match latex_arrow(&source[start..end]) {
                    Some(symbol) => {
                        output.push_str(symbol);
                        index = end;
                    }
                    None => {
                        output.push('\\');
                        index += 1;
                    }
                }
            }
            _ => {
                output.push(character);
                index += character.len_utf8();
            }
        }
    }
    output
}

/// Parse blocks separately because Slint's inline renderer does not support headings,
/// fenced code, tables or quotes. The source string is never changed or used for copying.
fn render(source: &str) -> Vec<MarkdownBlock> {
    let display_source = cjk_emphasis(&latex_arrows(source));
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    let mut blocks = Vec::new();
    let mut current = block();
    let mut inline = Inline::default();
    let mut cells = Vec::new();
    let mut table_rows: Vec<Vec<MarkdownCell>> = Vec::new();
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut quote_depth = 0;
    let mut in_table = false;
    let mut in_table_head = false;

    fn flush(blocks: &mut Vec<MarkdownBlock>, current: &mut MarkdownBlock, inline: &mut Inline) {
        if !inline.plain.is_empty() || current.code {
            current.cells = model(vec![inline.cell(current.code, false)]);
            blocks.push(current.clone());
        }
        // Keep the container context for continuation paragraphs and nested blocks.
        current.prefix = "".into();
        current.font_scale = 1.0;
        current.heading = false;
        current.code = false;
    }

    // Renders a whole table as one block: every row shares a single grid so the
    // column widths line up, and short rows are padded to the header's columns.
    fn finish_table(
        blocks: &mut Vec<MarkdownBlock>,
        current: &mut MarkdownBlock,
        rows: &mut Vec<Vec<MarkdownCell>>,
    ) {
        let columns = rows.iter().map(|row| row.len()).max().unwrap_or(0).max(1);
        let table_rows: Vec<MarkdownRow> = rows
            .drain(..)
            .map(|mut row| {
                row.resize_with(columns, || MarkdownCell {
                    content: StyledText::from_plain_text(""),
                    heading: false,
                });
                MarkdownRow { cells: model(row) }
            })
            .collect();
        if !table_rows.is_empty() {
            current.columns = columns as i32;
            current.rows = model(table_rows);
            blocks.push(current.clone());
        }
        current.table = false;
        current.columns = 0;
        current.rows = Default::default();
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
                current.table = true;
            }
            Event::Start(Tag::TableHead) => in_table_head = true,
            Event::End(TagEnd::TableCell) => cells.push(inline.cell(false, in_table_head)),
            Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
                table_rows.push(std::mem::take(&mut cells));
                in_table_head = false;
            }
            Event::End(TagEnd::Table) => {
                in_table = false;
                finish_table(&mut blocks, &mut current, &mut table_rows);
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
    if in_table {
        finish_table(&mut blocks, &mut current, &mut table_rows);
    } else {
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
        // A table becomes a single block: its rows share one grid so the columns
        // line up, and the header cells carry bold content and the heading flag.
        let table = &blocks[5];
        assert!(table.table);
        assert_eq!(table.columns, 2);
        assert_eq!(table.rows.row_count(), 2);
        let header = table.rows.row_data(0).unwrap();
        assert!(header.cells.row_data(0).unwrap().heading);
        assert_eq!(
            header.cells.row_data(0).unwrap().content,
            StyledText::from_markdown("**A**").unwrap()
        );
        let body = table.rows.row_data(1).unwrap();
        assert!(!body.cells.row_data(0).unwrap().heading);
        assert_eq!(
            body.cells.row_data(0).unwrap().content,
            StyledText::from_markdown("**one**").unwrap()
        );
        assert_eq!(
            body.cells.row_data(1).unwrap().content,
            StyledText::from_markdown("two").unwrap()
        );
        assert!(blocks[6].rule);
    }

    #[test]
    fn table_rows_are_padded_to_the_header_column_count() {
        let blocks = render("| A | B |
| - | - |
| only |");
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].table);
        assert_eq!(blocks[0].columns, 2);
        assert_eq!(blocks[0].rows.row_count(), 2);
        assert_eq!(blocks[0].rows.row_data(1).unwrap().cells.row_count(), 2);
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
        // Long results must scroll: the block rows clamp their minimum height to 0px, which
        // defeats the flickable's automatic viewport sizing, so the ScrollView binds its
        // viewport-height to the wrapped content height.
        {
            let (width, height) = (600u32, 260u32);
            surface.set_size(slint::PhysicalSize::new(width, height));
            let render = |window: &TextboxWindow| {
                window.window().request_redraw();
                let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
                assert!(surface.draw_if_needed(|renderer| {
                    renderer.render(&mut pixels, width as usize);
                }));
                pixels
            };
            let before = render(&window);
            window.window().dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(300., 150.),
                delta_x: 0.,
                delta_y: -120.,
            });
            let after = render(&window);
            let moved = before.iter().zip(after.iter()).filter(|(a, b)| a != b).count();
            assert!(moved > 0, "the mouse wheel must scroll the result window");
        }
        window.set_text("".into());
        assert_eq!(window.get_text(), "");
        window.hide().unwrap();
    }

    #[test]
    fn latex_arrows_in_math_spans_become_unicode_arrows() {
        for (source, expected) in [
            (r"$\leftarrow$", "←"),
            (r"$\rightarrow$", "→"),
            (r"$\to$와 $\gets$", "→와 ←"),
            (r"$\Leftrightarrow$", "⇔"),
            (r"$\Longrightarrow$", "⟹"),
            (r"$\mapsto$", "↦"),
            (r"$\uparrow \downarrow$", "↑ ↓"),
            (r"$x \to y$", "x → y"),
            (r"A \rightarrow B", "A → B"),
        ] {
            assert_eq!(latex_arrows(source), expected, "{source}");
        }
        assert_eq!(
            content(&render(r"A $\leftarrow$ B")[0], 0),
            StyledText::from_plain_text("A ← B")
        );
    }

    #[test]
    fn latex_arrows_leave_code_currency_and_unknown_commands_alone() {
        for source in [
            r"가격은 $5",
            r"$5 and $10",
            r"`$\leftarrow$`",
            "```\n$\\leftarrow$\n```",
            r"$\alpha$",
            r"\leftarrowX",
        ] {
            assert_eq!(latex_arrows(source), source);
        }
    }

}
