use anstyle::{AnsiColor, Style};
use std::{
    env,
    ffi::OsStr,
    fmt::Display,
    io::{self, IsTerminal},
    str::FromStr,
    sync::OnceLock,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

impl FromStr for ColorChoice {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            _ => Err("expected `auto`, `always`, or `never`"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Title,
    Heading,
    Label,
    Success,
    Warning,
    Error,
    Muted,
    Accent,
    Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalStyle {
    color: bool,
}

impl TerminalStyle {
    #[cfg(test)]
    pub(crate) const fn plain() -> Self {
        Self { color: false }
    }

    #[cfg(test)]
    pub(crate) const fn colored() -> Self {
        Self { color: true }
    }

    pub(crate) fn paint(&self, role: Role, value: impl Display) -> String {
        let value = value.to_string();
        if !self.color {
            return value;
        }
        let style = style_for(role);
        format!("{}{value}{}", style.render(), style.render_reset())
    }

    pub(crate) fn title(&self, value: impl Display) -> String {
        self.paint(Role::Title, value)
    }

    pub(crate) fn heading(&self, value: impl Display) -> String {
        self.paint(Role::Heading, value)
    }

    pub(crate) fn label(&self, value: impl Display) -> String {
        self.paint(Role::Label, value)
    }

    pub(crate) fn success(&self, value: impl Display) -> String {
        self.paint(Role::Success, value)
    }

    pub(crate) fn warning(&self, value: impl Display) -> String {
        self.paint(Role::Warning, value)
    }

    pub(crate) fn error(&self, value: impl Display) -> String {
        self.paint(Role::Error, value)
    }

    pub(crate) fn muted(&self, value: impl Display) -> String {
        self.paint(Role::Muted, value)
    }

    pub(crate) fn accent(&self, value: impl Display) -> String {
        self.paint(Role::Accent, value)
    }

    pub(crate) fn command(&self, value: impl Display) -> String {
        self.paint(Role::Command, value)
    }

    pub(crate) fn semantic(&self, value: impl Display) -> String {
        let value = value.to_string();
        let normalized = value.trim().to_ascii_lowercase();
        let role = match normalized.as_str() {
            "allow" | "active" | "approved" | "authorized" | "compatible" | "created"
            | "enabled" | "external_ready" | "healthy" | "live_ready" | "ok" | "owned_running"
            | "pass" | "ready" | "running_ready" | "settled" | "succeeded" | "true" => {
                Role::Success
            }
            "deny"
            | "error"
            | "external_unavailable"
            | "fail"
            | "failed"
            | "incompatible"
            | "invalid"
            | "owned_exited"
            | "owned_unhealthy"
            | "denied"
            | "expired"
            | "revoked"
            | "stale_identity" => Role::Error,
            "claimed" | "disabled" | "drift" | "fixture_only" | "frozen" | "incomplete"
            | "needs_approval" | "pending" | "released" | "ready_to_render" | "ready_to_start"
            | "warning" => Role::Warning,
            "client_owned" | "false" | "not configured" | "not rendered" | "skipped"
            | "stopped" | "unconfigured" | "unknown" => Role::Muted,
            _ => Role::Accent,
        };
        self.paint(role, value)
    }
}

static COLOR_CHOICE: OnceLock<ColorChoice> = OnceLock::new();

pub(crate) fn configure(choice: ColorChoice) {
    let _ = COLOR_CHOICE.set(choice);
}

pub(crate) fn stdout() -> TerminalStyle {
    style_for_stream(io::stdout().is_terminal())
}

pub(crate) fn stderr() -> TerminalStyle {
    style_for_stream(io::stderr().is_terminal())
}

fn style_for_stream(is_terminal: bool) -> TerminalStyle {
    TerminalStyle {
        color: color_enabled(
            *COLOR_CHOICE.get().unwrap_or(&ColorChoice::Auto),
            is_terminal,
            env::var_os("NO_COLOR").as_deref(),
            env::var_os("TERM").as_deref(),
        ),
    }
}

fn color_enabled(
    choice: ColorChoice,
    is_terminal: bool,
    no_color: Option<&OsStr>,
    term: Option<&OsStr>,
) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => {
            is_terminal
                && no_color.is_none_or(|value| value.is_empty())
                && term.is_none_or(|value| value != OsStr::new("dumb"))
        }
    }
}

fn style_for(role: Role) -> Style {
    match role {
        Role::Title => Style::new()
            .fg_color(Some(AnsiColor::BrightCyan.into()))
            .bold(),
        Role::Heading | Role::Label => Style::new().bold(),
        Role::Success => Style::new().fg_color(Some(AnsiColor::Green.into())).bold(),
        Role::Warning => Style::new().fg_color(Some(AnsiColor::Yellow.into())).bold(),
        Role::Error => Style::new().fg_color(Some(AnsiColor::Red.into())).bold(),
        Role::Muted => Style::new().dimmed(),
        Role::Accent => Style::new().fg_color(Some(AnsiColor::BrightCyan.into())),
        Role::Command => Style::new().fg_color(Some(AnsiColor::Cyan.into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_color_choices() {
        assert_eq!("auto".parse(), Ok(ColorChoice::Auto));
        assert_eq!("always".parse(), Ok(ColorChoice::Always));
        assert_eq!("never".parse(), Ok(ColorChoice::Never));
        assert_eq!(
            "sometimes".parse::<ColorChoice>(),
            Err("expected `auto`, `always`, or `never`")
        );
    }

    #[test]
    fn auto_color_requires_a_terminal() {
        assert!(color_enabled(ColorChoice::Auto, true, None, None));
        assert!(!color_enabled(ColorChoice::Auto, false, None, None));
    }

    #[test]
    fn no_color_disables_automatic_color_when_non_empty() {
        assert!(!color_enabled(
            ColorChoice::Auto,
            true,
            Some(OsStr::new("1")),
            None
        ));
        assert!(color_enabled(
            ColorChoice::Auto,
            true,
            Some(OsStr::new("")),
            None
        ));
        assert!(color_enabled(
            ColorChoice::Always,
            true,
            Some(OsStr::new("1")),
            None
        ));
    }

    #[test]
    fn dumb_terminal_disables_automatic_color_only() {
        assert!(!color_enabled(
            ColorChoice::Auto,
            true,
            None,
            Some(OsStr::new("dumb"))
        ));
        assert!(color_enabled(
            ColorChoice::Always,
            true,
            None,
            Some(OsStr::new("dumb"))
        ));
    }

    #[test]
    fn colored_and_plain_styles_preserve_text() {
        let plain = TerminalStyle::plain().success("running_ready");
        let colored = TerminalStyle::colored().success("running_ready");
        assert_eq!(plain, "running_ready");
        assert!(colored.contains("\u{1b}["));
        assert!(colored.contains("running_ready"));
        assert!(colored.ends_with("\u{1b}[0m"));
    }

    #[test]
    fn semantic_roles_have_distinct_terminal_styles() {
        let style = TerminalStyle::colored();
        let success = style.success("state");
        let warning = style.warning("state");
        let error = style.error("state");
        let muted = style.muted("state");
        let accent = style.accent("state");

        for (left, right) in [
            (&success, &warning),
            (&success, &error),
            (&success, &muted),
            (&success, &accent),
            (&warning, &error),
            (&warning, &muted),
            (&warning, &accent),
            (&error, &muted),
            (&error, &accent),
            (&muted, &accent),
        ] {
            assert_ne!(left, right);
        }
        assert_eq!(style.semantic("pass"), style.success("pass"));
        assert_eq!(style.semantic("warning"), style.warning("warning"));
        assert_eq!(style.semantic("failed"), style.error("failed"));
        assert_eq!(style.semantic("stopped"), style.muted("stopped"));
    }
}
