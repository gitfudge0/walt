#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThemeKind {
    System,
    CatppuccinMocha,
    TokyoNight,
    GruvboxDark,
    Dracula,
    Nord,
    SolarizedDark,
    Kanagawa,
    OneDark,
    EverforestDark,
    RosePine,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 11] = [
        ThemeKind::System,
        ThemeKind::CatppuccinMocha,
        ThemeKind::TokyoNight,
        ThemeKind::GruvboxDark,
        ThemeKind::Dracula,
        ThemeKind::Nord,
        ThemeKind::SolarizedDark,
        ThemeKind::Kanagawa,
        ThemeKind::OneDark,
        ThemeKind::EverforestDark,
        ThemeKind::RosePine,
    ];

    pub fn from_name(name: &str) -> Self {
        match name {
            "Catppuccin Mocha" => Self::CatppuccinMocha,
            "Tokyo Night" => Self::TokyoNight,
            "Gruvbox Dark" => Self::GruvboxDark,
            "Dracula" => Self::Dracula,
            "Nord" => Self::Nord,
            "Solarized Dark" => Self::SolarizedDark,
            "Kanagawa" => Self::Kanagawa,
            "One Dark" => Self::OneDark,
            "Everforest Dark" => Self::EverforestDark,
            "Rosé Pine" => Self::RosePine,
            _ => Self::System,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::TokyoNight => "Tokyo Night",
            Self::GruvboxDark => "Gruvbox Dark",
            Self::Dracula => "Dracula",
            Self::Nord => "Nord",
            Self::SolarizedDark => "Solarized Dark",
            Self::Kanagawa => "Kanagawa",
            Self::OneDark => "One Dark",
            Self::EverforestDark => "Everforest Dark",
            Self::RosePine => "Rosé Pine",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|theme| *theme == self)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::ThemeKind;

    #[test]
    fn theme_names_round_trip() {
        for theme in ThemeKind::ALL {
            assert_eq!(ThemeKind::from_name(theme.name()), theme);
        }
    }
}
