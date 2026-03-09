use crate::theme::ThemeKind;
use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy)]
pub struct ThemePalette {
    pub surface: Style,
    pub border: Style,
    pub title: Style,
    pub accent: Style,
    pub muted: Style,
    pub highlight: Style,
    pub key: Style,
    pub placeholder: Style,
}

impl ThemeKind {
    pub fn palette(self) -> ThemePalette {
        match self {
            Self::System => ThemePalette {
                surface: Style::default(),
                border: Style::default(),
                title: Style::default().add_modifier(Modifier::BOLD),
                accent: Style::default().add_modifier(Modifier::BOLD),
                muted: Style::default().add_modifier(Modifier::DIM),
                highlight: Style::default()
                    .fg(selection_color())
                    .add_modifier(Modifier::BOLD),
                key: Style::default().add_modifier(Modifier::BOLD),
                placeholder: Style::default().add_modifier(Modifier::DIM),
            },
            Self::CatppuccinMocha => palette(
                rgb(30, 30, 46),
                rgb(203, 166, 247),
                rgb(249, 226, 175),
                rgb(205, 214, 244),
                rgb(108, 112, 134),
                rgb(49, 50, 68),
                rgb(137, 180, 250),
            ),
            Self::TokyoNight => palette(
                rgb(26, 27, 38),
                rgb(122, 162, 247),
                rgb(187, 154, 247),
                rgb(192, 202, 245),
                rgb(86, 95, 137),
                rgb(41, 46, 66),
                rgb(125, 207, 255),
            ),
            Self::GruvboxDark => palette(
                rgb(40, 40, 40),
                rgb(250, 189, 47),
                rgb(184, 187, 38),
                rgb(235, 219, 178),
                rgb(146, 131, 116),
                rgb(60, 56, 54),
                rgb(131, 165, 152),
            ),
            Self::Dracula => palette(
                rgb(40, 42, 54),
                rgb(189, 147, 249),
                rgb(255, 184, 108),
                rgb(248, 248, 242),
                rgb(98, 114, 164),
                rgb(68, 71, 90),
                rgb(139, 233, 253),
            ),
            Self::Nord => palette(
                rgb(46, 52, 64),
                rgb(136, 192, 208),
                rgb(163, 190, 140),
                rgb(229, 233, 240),
                rgb(94, 129, 172),
                rgb(59, 66, 82),
                rgb(180, 142, 173),
            ),
            Self::SolarizedDark => palette(
                rgb(0, 43, 54),
                rgb(38, 139, 210),
                rgb(181, 137, 0),
                rgb(238, 232, 213),
                rgb(101, 123, 131),
                rgb(7, 54, 66),
                rgb(42, 161, 152),
            ),
            Self::Kanagawa => palette(
                rgb(31, 31, 40),
                rgb(125, 207, 255),
                rgb(152, 187, 108),
                rgb(220, 215, 186),
                rgb(114, 135, 135),
                rgb(34, 42, 54),
                rgb(255, 160, 102),
            ),
            Self::OneDark => palette(
                rgb(40, 44, 52),
                rgb(97, 175, 239),
                rgb(198, 120, 221),
                rgb(171, 178, 191),
                rgb(92, 99, 112),
                rgb(40, 44, 52),
                rgb(224, 108, 117),
            ),
            Self::EverforestDark => palette(
                rgb(45, 53, 47),
                rgb(163, 190, 140),
                rgb(230, 197, 71),
                rgb(211, 198, 170),
                rgb(122, 131, 113),
                rgb(45, 53, 47),
                rgb(127, 187, 179),
            ),
            Self::RosePine => palette(
                rgb(25, 23, 36),
                rgb(196, 167, 231),
                rgb(235, 188, 186),
                rgb(224, 222, 244),
                rgb(144, 140, 170),
                rgb(38, 35, 58),
                rgb(156, 207, 216),
            ),
        }
    }
}

fn palette(
    surface: Color,
    border: Color,
    title: Color,
    text: Color,
    muted: Color,
    _highlight: Color,
    key: Color,
) -> ThemePalette {
    ThemePalette {
        surface: Style::default().bg(surface),
        border: Style::default().fg(border).bg(surface),
        title: Style::default()
            .fg(title)
            .bg(surface)
            .add_modifier(Modifier::BOLD),
        accent: Style::default().fg(text).bg(surface),
        muted: Style::default().fg(muted).bg(surface),
        highlight: Style::default()
            .fg(selection_color())
            .bg(surface)
            .add_modifier(Modifier::BOLD),
        key: Style::default()
            .fg(key)
            .bg(surface)
            .add_modifier(Modifier::BOLD),
        placeholder: Style::default()
            .fg(muted)
            .bg(surface)
            .add_modifier(Modifier::ITALIC),
    }
}

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

fn selection_color() -> Color {
    Color::Rgb(255, 215, 64)
}
