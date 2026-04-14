use crate::input;

use ratatui::{layout::{Constraint, Layout}, style::Styled, text::ToLine, widgets::Widget};

const RED: ratatui::style::Color = ratatui::style::Color::Red;
const BLACK: ratatui::style::Color = ratatui::style::Color::Black;
const GRAY: ratatui::style::Color = ratatui::style::Color::DarkGray;

const BOLD: ratatui::style::Style = ratatui::style::Style::new().fg(BLACK).bold();
const WEAK: ratatui::style::Style = ratatui::style::Style::new().fg(GRAY).bold();
const BG: ratatui::style::Style = ratatui::style::Style::new().bg(RED);

impl super::App {
    fn render_footer(&self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        let [_, help_area, num_area] = area.layout(&Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ]));
        let mut help = ratatui::widgets::Block::new().set_style(BG);
        if !self.dialog.open {
            help = help.title(
                " ` to open audio config | esc to exit program "
                    .to_line()
                    .centered()
                    .patch_style(BOLD),
            )
        }
        let [num_area] = num_area.layout(&Layout::horizontal([Constraint::Fill(1)]).horizontal_margin(14));
        help.render(help_area, buf);
        let string = self.num_buffer.num.to_string();
        ratatui::widgets::Block::new().title(string.to_line().right_aligned().patch_style(WEAK)).render(num_area, buf);
    }

    fn render_phrase(&self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        let [_, area, _] = area.layout(&Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(4),
            Constraint::Length(4),
        ]));
        let area = area.centered_horizontally(Constraint::Length(7 * input::PHRASE_COUNT as u16 - 1));
        let areas: [_; input::PHRASE_COUNT] = area.layout(&Layout::horizontal([Constraint::Length(6); input::PHRASE_COUNT]).spacing(1));
        let mask = &self.state_handler.get_record_mask();
        for (i, area) in areas.into_iter().enumerate() {
            let masked = mask.contains(&(i as u8));
            let mut block = ratatui::widgets::Block::new().style(BG);
            if masked {
                block = block.borders(ratatui::widgets::Borders::ALL).border_style(BOLD);
            }
            block.render(area, buf);
            if let Some(ttcore::state::Phrase { start, len, .. }) = self.state_handler.phrases[i] {
                let mut lines = vec![start.to_string(), len.to_string()];
                if let Some(tick) = self.state_handler.get_reader_tick(i as u8) {
                    lines.push(tick.to_string());
                }
                ratatui::widgets::Paragraph::new(
                    ratatui::text::Text::from_iter(lines.into_iter())
                ).set_style(WEAK).render(area, buf);
            }
        }
    }
}

impl Widget for &input::App {
    fn render(self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        self.render_footer(area, buf);
        self.throbber.render(area, buf);
        self.render_phrase(area, buf);
        if self.dialog.open {
            self.dialog.render(area, buf);
        }
    }
}

impl Widget for &input::Throbber {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer) {
        let area = area.centered_horizontally(Constraint::Length(6));
        let [step_area, throbber_area] = area.layout(&Layout::vertical([Constraint::Length(1),Constraint::Length(3)]).flex(ratatui::layout::Flex::Center));
        ratatui::widgets::Paragraph::new(
            ratatui::text::Text::from((self.step + 1).to_string()),
        ).set_style(WEAK).render(step_area, buf);
        if self.high {
            ratatui::widgets::Block::new().style(BG).render(throbber_area, buf);
        } else {
            ratatui::widgets::Clear.render(throbber_area, buf);
        }
    }
}

impl<T: ratatui::text::ToLine> Widget for &input::Scroll<T> {
    fn render(self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        if self.list.is_empty() {
            let inner = area.centered_vertically(Constraint::Length(1));
            ratatui::widgets::Paragraph::new("no entries found :/".set_style(BOLD))
                .render(inner, buf);
        } else {
            let iter = std::iter::repeat_n(ratatui::text::Line::raw(""), 3)
                .chain(self.list.iter().map(|v| v.to_line().patch_style(WEAK)))
                .chain(std::iter::repeat_n(ratatui::text::Line::raw(""), 3));
            let mut text = ratatui::text::Text::from_iter(iter);
            text.lines[self.index + 3] = text.lines[self.index + 3].clone().patch_style(BOLD);
            ratatui::widgets::Paragraph::new(text)
                .scroll((self.index as u16, 0))
                .render(area, buf);
        }
    }
}

impl Widget for &input::CpalConfigDialog {
    fn render(self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        let area = area.centered(Constraint::Percentage(80), Constraint::Length(9));
        let inner = area.inner(ratatui::layout::Margin::new(1, 1));
        ratatui::widgets::Clear.render(area, buf);
        let mut block = ratatui::widgets::Block::bordered()
            .border_style(BOLD)
            .title_style(BOLD)
            .set_style(BG);
        match self.tab {
            input::Tab::Hosts => {
                block = block
                    .title_top(" select an audio host: ".to_line().centered())
                    .title_bottom(" devices \u{2192} ".to_line().right_aligned())
                    .title_bottom(" \u{2195} to scroll | esc to exit ".to_line().centered());
                self.hosts.render(inner, buf);
            }
            input::Tab::Devices => {
                block = block
                    .title_top(" select an audio device: ".to_line().centered())
                    .title_bottom(" \u{2190} hosts ".to_line().left_aligned())
                    .title_bottom(" \u{2195} to scroll | esc to exit ".to_line().centered())
                    .title_bottom(" commit \u{2192} ".to_line().right_aligned());
                self.devices.render(inner, buf);
            }
            input::Tab::Farewell(..) => {
                block = block
                    .title_bottom(" \u{2190} devices ".to_line().left_aligned())
                    .title_bottom(" esc to exit ".to_line().centered());
                let inner = inner.centered_vertically(Constraint::Length(1));
                ratatui::widgets::Paragraph::new("please make some noise <3".set_style(BOLD))
                    .render(inner, buf);
            }
        };
        block.render(area, buf);
    }
}
