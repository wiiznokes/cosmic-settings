// Copyright 2023 System76 <info@system76.com>
// SPDX-License-Identifier: GPL-3.0-only

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use ashpd::desktop::file_chooser::{FileFilter, SelectedFiles};
use cosmic::config::CosmicTk;
use cosmic::cosmic_config::{Config, ConfigSet, CosmicConfigEntry};
use cosmic::cosmic_theme::palette::{FromColor, Hsv, Srgb, Srgba};
use cosmic::cosmic_theme::{
    CornerRadii, Theme, ThemeBuilder, ThemeMode, DARK_THEME_BUILDER_ID, LIGHT_THEME_BUILDER_ID,
};
use cosmic::iced_core::{alignment, Background, Color, Length};
use cosmic::iced_widget::scrollable;
use cosmic::prelude::CollectionWidget;
use cosmic::widget::icon::{self, from_name, icon};
use cosmic::widget::{
    button, color_picker::ColorPickerUpdate, container, flex_row, horizontal_space, row, settings,
    spin_button, text, ColorPickerModel,
};
use cosmic::Apply;
use cosmic::{command, Command, Element};
use cosmic_panel_config::CosmicPanelConfig;
use cosmic_settings_page::Section;
use cosmic_settings_page::{self as page, section};
use cosmic_settings_wallpaper as wallpaper;
use ron::ser::PrettyConfig;
use slab::Slab;
use slotmap::SlotMap;
use tokio::io::AsyncBufReadExt;

use crate::app;

use super::wallpaper::widgets::color_image;

const ICON_PREV_N: usize = 6;
const ICON_PREV_ROW: usize = 3;
const ICON_TRY_SIZES: [u16; 3] = [32, 48, 64];
const ICON_THUMB_SIZE: u16 = 32;
const ICON_NAME_TRUNC: usize = 20;

pub type IconThemes = Vec<IconTheme>;
pub type IconHandles = Vec<[icon::Handle; ICON_PREV_N]>;

crate::cache_dynamic_lazy! {
    static HEX: String = fl!("hex");
    static RGB: String = fl!("rgb");
    static RESET_TO_DEFAULT: String = fl!("reset-to-default");
    static ICON_THEME: String = fl!("icon-theme");
}

#[derive(Clone, Copy, Debug)]
enum ContextView {
    AccentWindowHint,
    ApplicationBackground,
    ContainerBackground,
    ControlComponent,
    CustomAccent,
    Experimental,
    InterfaceText,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct IconTheme {
    // COSMIC uses the file name of the folder containing the theme
    id: String,
    // GTK uses the name of the theme as specified in its index file
    name: String,
}

pub struct Page {
    can_reset: bool,
    no_custom_window_hint: bool,
    context_view: Option<ContextView>,
    custom_accent: ColorPickerModel,
    accent_window_hint: ColorPickerModel,
    application_background: ColorPickerModel,
    container_background: ColorPickerModel,
    interface_text: ColorPickerModel,
    control_component: ColorPickerModel,
    roundness: Roundness,

    icon_theme_active: Option<usize>,
    icon_themes: IconThemes,
    icon_handles: IconHandles,

    theme_mode: ThemeMode,
    theme_mode_config: Option<Config>,
    theme_builder: ThemeBuilder,
    theme_builder_needs_update: bool,
    theme_builder_config: Option<Config>,

    auto_switch_descs: [Cow<'static, str>; 4],

    tk: CosmicTk,
    tk_config: Option<Config>,

    day_time: bool,
}

impl Default for Page {
    fn default() -> Self {
        let theme_mode_config = ThemeMode::config().ok();
        let theme_mode = theme_mode_config
            .as_ref()
            .map(|c| match ThemeMode::get_entry(c) {
                Ok(t) => t,
                Err((errors, t)) => {
                    for e in errors {
                        tracing::error!("{e}");
                    }
                    t
                }
            })
            .unwrap_or_default();

        (theme_mode_config, theme_mode).into()
    }
}

impl
    From<(
        Option<Config>,
        ThemeMode,
        Option<Config>,
        ThemeBuilder,
        Option<Config>,
        CosmicTk,
    )> for Page
{
    fn from(
        (theme_mode_config, theme_mode, theme_builder_config, theme_builder, tk_config, tk): (
            Option<Config>,
            ThemeMode,
            Option<Config>,
            ThemeBuilder,
            Option<Config>,
            CosmicTk,
        ),
    ) -> Self {
        let theme = if theme_mode.is_dark {
            Theme::dark_default()
        } else {
            Theme::light_default()
        };
        let custom_accent = theme_builder.accent.filter(|c| {
            let c = Srgba::new(c.red, c.green, c.blue, 1.0);
            c != theme.palette.accent_blue
                && c != theme.palette.accent_green
                && c != theme.palette.accent_indigo
                && c != theme.palette.accent_orange
                && c != theme.palette.accent_pink
                && c != theme.palette.accent_purple
                && c != theme.palette.accent_red
                && c != theme.palette.accent_warm_grey
                && c != theme.palette.accent_yellow
        });

        Self {
            can_reset: if theme_mode.is_dark {
                theme_builder == ThemeBuilder::dark()
            } else {
                theme_builder == ThemeBuilder::light()
            },
            theme_builder_needs_update: false,
            context_view: None,
            roundness: theme_builder.corner_radii.into(),
            custom_accent: ColorPickerModel::new(
                &*HEX,
                &*RGB,
                None,
                custom_accent.map(Color::from),
            ),
            application_background: ColorPickerModel::new(
                &*HEX,
                &*RGB,
                Some(theme.background.base.into()),
                theme_builder.bg_color.map(Color::from),
            ),
            container_background: ColorPickerModel::new(
                &*HEX,
                &*RGB,
                None,
                theme_builder.primary_container_bg.map(Color::from),
            ),
            interface_text: ColorPickerModel::new(
                &*HEX,
                &*RGB,
                Some(theme.background.on.into()),
                theme_builder.text_tint.map(Color::from),
            ),
            control_component: ColorPickerModel::new(
                &*HEX,
                &*RGB,
                Some(theme.palette.neutral_5.into()),
                theme_builder.neutral_tint.map(Color::from),
            ),
            accent_window_hint: ColorPickerModel::new(
                &*HEX,
                &*RGB,
                None,
                theme_builder.window_hint.map(Color::from),
            ),
            no_custom_window_hint: theme_builder.accent.is_some(),
            icon_theme_active: None,
            icon_themes: Vec::new(),
            icon_handles: Vec::new(),
            theme_mode_config,
            theme_builder_config,
            theme_mode,
            theme_builder,
            tk_config,
            tk,
            day_time: true,
            auto_switch_descs: [
                fl!("auto-switch", "sunrise").into(),
                fl!("auto-switch", "sunset").into(),
                fl!("auto-switch", "next-sunrise").into(),
                fl!("auto-switch", "next-sunset").into(),
            ],
        }
    }
}

impl From<(Option<Config>, ThemeMode)> for Page {
    fn from((theme_mode_config, theme_mode): (Option<Config>, ThemeMode)) -> Self {
        let theme_builder_config = if theme_mode.is_dark {
            ThemeBuilder::dark_config()
        } else {
            ThemeBuilder::light_config()
        }
        .ok();
        let theme_builder = theme_builder_config.as_ref().map_or_else(
            || {
                if theme_mode.is_dark {
                    ThemeBuilder::dark()
                } else {
                    ThemeBuilder::light()
                }
            },
            |c| match ThemeBuilder::get_entry(c) {
                Ok(t) => t,
                Err((errors, t)) => {
                    for e in errors {
                        tracing::error!("{e}");
                    }
                    t
                }
            },
        );

        let tk_config = CosmicTk::config().ok();
        let tk = match tk_config.as_ref().map(CosmicTk::get_entry) {
            Some(Ok(c)) => c,
            Some(Err((errs, c))) => {
                for err in errs {
                    tracing::error!(?err, "Error loading CosmicTk");
                }
                c
            }
            None => CosmicTk::default(),
        };
        (
            theme_mode_config,
            theme_mode,
            theme_builder_config,
            theme_builder,
            tk_config,
            tk,
        )
            .into()
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    AccentWindowHint(ColorPickerUpdate),
    ApplicationBackground(ColorPickerUpdate),
    ApplyThemeGlobal(bool),
    Autoswitch(bool),
    ContainerBackground(ColorPickerUpdate),
    ControlComponent(ColorPickerUpdate),
    CustomAccent(ColorPickerUpdate),
    DarkMode(bool),
    Entered((IconThemes, IconHandles)),
    ExperimentalContextDrawer,
    ExportError,
    ExportFile(Arc<SelectedFiles>),
    ExportSuccess,
    GapSize(spin_button::Message),
    IconTheme(usize),
    ImportError,
    ImportFile(Arc<SelectedFiles>),
    ImportSuccess(Box<ThemeBuilder>),
    InterfaceText(ColorPickerUpdate),
    Left,
    PaletteAccent(cosmic::iced::Color),
    Reset,
    Roundness(Roundness),
    StartExport,
    StartImport,
    UseDefaultWindowHint(bool),
    WindowHintSize(spin_button::Message),
    Daytime(bool),
}

#[derive(Debug, Clone, Copy)]
pub enum Roundness {
    Round,
    SlightlyRound,
    Square,
}

impl From<Roundness> for CornerRadii {
    fn from(value: Roundness) -> Self {
        match value {
            Roundness::Round => CornerRadii {
                radius_0: [0.0; 4],
                radius_xs: [4.0; 4],
                radius_s: [8.0; 4],
                radius_m: [16.0; 4],
                radius_l: [32.0; 4],
                radius_xl: [160.0; 4],
            },
            Roundness::SlightlyRound => CornerRadii {
                radius_0: [0.0; 4],
                radius_xs: [2.0; 4],
                radius_s: [8.0; 4],
                radius_m: [8.0; 4],
                radius_l: [8.0; 4],
                radius_xl: [8.0; 4],
            },
            Roundness::Square => CornerRadii {
                radius_0: [0.0; 4],
                radius_xs: [2.0; 4],
                radius_s: [2.0; 4],
                radius_m: [2.0; 4],
                radius_l: [2.0; 4],
                radius_xl: [2.0; 4],
            },
        }
    }
}

impl From<CornerRadii> for Roundness {
    fn from(value: CornerRadii) -> Self {
        if (value.radius_m[0] - 16.0).abs() < 0.01 {
            Self::Round
        } else if (value.radius_m[0] - 8.0).abs() < 0.01 {
            Self::SlightlyRound
        } else {
            Self::Square
        }
    }
}

impl Page {
    /// Syncs changes for dark and light theme.
    /// Roundness and window management settings should be consistent between dark / light mode.
    fn sync_changes(&self) -> Result<(), cosmic::cosmic_config::Error> {
        let (other_builder_config, other_theme_config) = if self.theme_mode.is_dark {
            (ThemeBuilder::light_config()?, Theme::light_config()?)
        } else {
            (ThemeBuilder::dark_config()?, Theme::dark_config()?)
        };

        let mut theme_builder = match ThemeBuilder::get_entry(&other_builder_config) {
            Ok(t) => t,
            Err((errs, t)) => {
                for err in errs {
                    tracing::error!(?err, "Error loading theme builder");
                }
                t
            }
        };
        let mut theme = match Theme::get_entry(&other_theme_config) {
            Ok(t) => t,
            Err((errs, t)) => {
                for err in errs {
                    tracing::error!(?err, "Error loading theme");
                }
                t
            }
        };
        if theme_builder.active_hint != self.theme_builder.active_hint {
            if let Err(err) =
                theme_builder.set_active_hint(&other_builder_config, self.theme_builder.active_hint)
            {
                tracing::error!(?err, "Error setting active hint");
            }
            if let Err(err) =
                theme.set_active_hint(&other_theme_config, self.theme_builder.active_hint)
            {
                tracing::error!(?err, "Error setting active hint");
            }
        }
        if theme_builder.gaps != self.theme_builder.gaps {
            if let Err(err) = theme_builder.set_gaps(&other_builder_config, self.theme_builder.gaps)
            {
                tracing::error!(?err, "Error setting gaps");
            }
            if let Err(err) = theme.set_gaps(&other_theme_config, self.theme_builder.gaps) {
                tracing::error!(?err, "Error setting gaps");
            }
        }
        if theme_builder.corner_radii != self.theme_builder.corner_radii {
            if let Err(err) = theme_builder
                .set_corner_radii(&other_builder_config, self.theme_builder.corner_radii)
            {
                tracing::error!(?err, "Error setting corner radii");
            }

            if let Err(err) =
                theme.set_corner_radii(&other_theme_config, self.theme_builder.corner_radii)
            {
                tracing::error!(?err, "Error setting corner radii");
            }
        }

        Ok(())
    }

    fn color_picker_context_view(
        &self,
        description: Option<Cow<'static, str>>,
        reset: Cow<'static, str>,
        on_update: fn(ColorPickerUpdate) -> Message,
        model: impl Fn(&Self) -> &ColorPickerModel,
    ) -> Element<'_, crate::pages::Message> {
        cosmic::widget::column()
            .push_maybe(description.map(|description| text::body(description).width(Length::Fill)))
            .push(
                model(self)
                    .builder(on_update)
                    .reset_label(reset)
                    .height(Length::Fixed(158.0))
                    .build(
                        fl!("recent-colors"),
                        fl!("copy-to-clipboard"),
                        fl!("copied-to-clipboard"),
                    )
                    .apply(container)
                    .width(Length::Fixed(248.0))
                    .align_x(alignment::Horizontal::Center)
                    .apply(container)
                    .width(Length::Fill)
                    .align_x(alignment::Horizontal::Center),
            )
            .padding(self.theme_builder.spacing.space_l)
            .align_items(cosmic::iced_core::Alignment::Center)
            .spacing(self.theme_builder.spacing.space_m)
            .width(Length::Fill)
            .apply(Element::from)
            .map(crate::pages::Message::Appearance)
    }

    fn experimental_context_view(&self) -> Element<'_, crate::pages::Message> {
        let active = self.icon_theme_active;
        let theme = cosmic::theme::active();
        let theme = theme.cosmic();
        cosmic::iced::widget::column![
            // Export theme choice
            settings::view_section("").add(
                settings::item::builder(fl!("enable-export"))
                    .description(fl!("enable-export", "desc"))
                    .toggler(self.tk.apply_theme_global, Message::ApplyThemeGlobal)
            ),
            // Icon theme previews
            cosmic::widget::column::with_children(vec![
                text::heading(&*ICON_THEME).into(),
                flex_row(
                    self.icon_themes
                        .iter()
                        .zip(self.icon_handles.iter())
                        .enumerate()
                        .map(|(i, (theme, handles))| {
                            let selected = active.map(|j| i == j).unwrap_or_default();
                            icon_theme_button(&theme.name, handles, i, selected)
                        })
                        .collect(),
                )
                .row_spacing(theme.space_xs())
                .column_spacing(theme.space_xs())
                .into()
            ])
            .spacing(theme.space_xxs())
        ]
        .spacing(theme.space_m())
        .width(Length::Fill)
        .apply(Element::from)
        .map(crate::pages::Message::Appearance)
    }

    #[allow(clippy::too_many_lines)]
    pub fn update(&mut self, message: Message) -> Command<app::Message> {
        self.theme_builder_needs_update = false;
        let mut needs_sync = false;
        let ret = match message {
            Message::DarkMode(enabled) => {
                if let Some(config) = self.theme_mode_config.as_ref() {
                    if let Err(err) = self.theme_mode.set_is_dark(config, enabled) {
                        tracing::error!(?err, "Error setting dark mode");
                    }

                    self.reload_theme_mode();
                }

                Command::none()
            }
            Message::Autoswitch(enabled) => {
                self.theme_mode.auto_switch = enabled;
                if let Some(config) = self.theme_mode_config.as_ref() {
                    _ = config.set::<bool>("auto_switch", enabled);
                }
                Command::none()
            }
            Message::AccentWindowHint(u) => {
                needs_sync = true;
                let cmd = self.update_color_picker(
                    &u,
                    ContextView::AccentWindowHint,
                    fl!("window-hint-accent").into(),
                );
                Command::batch(vec![cmd, self.accent_window_hint.update::<app::Message>(u)])
            }
            Message::IconTheme(id) => {
                if let Some(theme) = self.icon_themes.get(id).cloned() {
                    self.icon_theme_active = Some(id);
                    self.tk.icon_theme = theme.id.clone();

                    if let Some(ref config) = self.tk_config {
                        let _ = self.tk.write_entry(config);
                    }

                    tokio::spawn(set_gnome_icon_theme(theme.name));
                }

                Command::none()
            }
            Message::WindowHintSize(msg) => {
                needs_sync = true;
                self.theme_builder_needs_update = true;
                self.theme_builder.active_hint = match msg {
                    spin_button::Message::Increment => {
                        self.theme_builder.active_hint.saturating_add(1)
                    }
                    spin_button::Message::Decrement => {
                        self.theme_builder.active_hint.saturating_sub(1)
                    }
                };
                Command::none()
            }
            Message::GapSize(msg) => {
                needs_sync = true;
                self.theme_builder_needs_update = true;
                self.theme_builder.gaps.1 = match msg {
                    spin_button::Message::Increment => self.theme_builder.gaps.1.saturating_add(1),
                    spin_button::Message::Decrement => self.theme_builder.gaps.1.saturating_sub(1),
                };
                Command::none()
            }
            Message::ApplicationBackground(u) => {
                let cmd = self.update_color_picker(
                    &u,
                    ContextView::ApplicationBackground,
                    fl!("app-background").into(),
                );

                Command::batch(vec![
                    cmd,
                    self.application_background.update::<app::Message>(u),
                ])
            }
            Message::ContainerBackground(u) => {
                let cmd = self.update_color_picker(
                    &u,
                    ContextView::ContainerBackground,
                    fl!("container-background").into(),
                );

                Command::batch(vec![
                    cmd,
                    self.container_background.update::<app::Message>(u),
                ])
            }
            Message::CustomAccent(u) => {
                let cmd = self.update_color_picker(
                    &u,
                    ContextView::CustomAccent,
                    fl!("accent-color").into(),
                );

                let cmd2 = self.custom_accent.update::<app::Message>(u);

                self.theme_builder.accent = self.custom_accent.get_applied_color().map(Srgb::from);
                Command::batch(vec![cmd, cmd2])
            }
            Message::InterfaceText(u) => {
                let cmd = self.update_color_picker(
                    &u,
                    ContextView::InterfaceText,
                    fl!("text-tint").into(),
                );

                Command::batch(vec![cmd, self.interface_text.update::<app::Message>(u)])
            }
            Message::ControlComponent(u) => {
                let cmd = self.update_color_picker(
                    &u,
                    ContextView::ControlComponent,
                    fl!("control-tint").into(),
                );
                Command::batch(vec![cmd, self.control_component.update::<app::Message>(u)])
            }
            Message::Roundness(r) => {
                needs_sync = true;
                self.roundness = r;
                self.theme_builder.corner_radii = self.roundness.into();
                self.theme_builder_needs_update = true;
                Self::update_panel_radii(r);
                Command::none()
            }
            Message::Entered((icon_themes, icon_handles)) => {
                *self = Self::default();

                // Set the icon themes, and define the active icon theme.
                self.icon_themes = icon_themes;
                self.icon_theme_active = self
                    .icon_themes
                    .iter()
                    .position(|theme| &theme.id == &self.tk.icon_theme);
                self.icon_handles = icon_handles;
                Command::none()
            }
            Message::Left => Command::perform(async {}, |()| {
                app::Message::SetTheme(cosmic::theme::system_preference())
            }),
            Message::PaletteAccent(c) => {
                self.theme_builder.accent = Some(c.into());
                self.theme_builder_needs_update = true;
                Command::none()
            }
            Message::Reset => {
                self.theme_builder = if self.theme_mode.is_dark {
                    cosmic::cosmic_config::Config::system(
                        DARK_THEME_BUILDER_ID,
                        ThemeBuilder::VERSION,
                    )
                    .map_or_else(
                        |_| ThemeBuilder::dark(),
                        |config| match ThemeBuilder::get_entry(&config) {
                            Ok(t) => t,
                            Err((errs, t)) => {
                                for err in errs {
                                    tracing::warn!(?err, "Error getting system theme builder");
                                }
                                t
                            }
                        },
                    )
                } else {
                    cosmic::cosmic_config::Config::system(
                        LIGHT_THEME_BUILDER_ID,
                        ThemeBuilder::VERSION,
                    )
                    .map_or_else(
                        |_| ThemeBuilder::light(),
                        |config| match ThemeBuilder::get_entry(&config) {
                            Ok(t) => t,
                            Err((errs, t)) => {
                                for err in errs {
                                    tracing::warn!(?err, "Error getting system theme builder");
                                }
                                t
                            }
                        },
                    )
                };
                if let Some(config) = self.theme_builder_config.as_ref() {
                    _ = self.theme_builder.write_entry(config);
                };

                let config = if self.theme_mode.is_dark {
                    Theme::dark_config()
                } else {
                    Theme::light_config()
                };
                let new_theme = self.theme_builder.clone().build();
                if let Ok(config) = config {
                    _ = new_theme.write_entry(&config);
                } else {
                    tracing::error!("Failed to get the theme config.");
                }

                Self::update_panel_radii(self.roundness);

                self.reload_theme_mode();
                Command::none()
            }
            Message::StartImport => Command::perform(
                async {
                    SelectedFiles::open_file()
                        .modal(true)
                        .filter(FileFilter::glob(FileFilter::new("ron"), "*.ron"))
                        .send()
                        .await?
                        .response()
                },
                |res| {
                    if let Ok(f) = res {
                        crate::Message::PageMessage(crate::pages::Message::Appearance(
                            Message::ImportFile(Arc::new(f)),
                        ))
                    } else {
                        // TODO Error toast?
                        tracing::error!("failed to select a file for importing a custom theme.");
                        crate::Message::PageMessage(crate::pages::Message::Appearance(
                            Message::ImportError,
                        ))
                    }
                },
            ),
            Message::StartExport => {
                let is_dark = self.theme_mode.is_dark;
                let name = format!("{}.ron", if is_dark { fl!("dark") } else { fl!("light") });
                Command::perform(
                    async move {
                        SelectedFiles::save_file()
                            .modal(true)
                            .current_name(Some(name.as_str()))
                            .send()
                            .await?
                            .response()
                    },
                    |res| {
                        if let Ok(f) = res {
                            crate::Message::PageMessage(crate::pages::Message::Appearance(
                                Message::ExportFile(Arc::new(f)),
                            ))
                        } else {
                            // TODO Error toast?
                            tracing::error!(
                                "failed to select a file for importing a custom theme."
                            );
                            crate::Message::PageMessage(crate::pages::Message::Appearance(
                                Message::ExportError,
                            ))
                        }
                    },
                )
            }
            Message::ImportFile(f) => {
                let Some(f) = f.uris().first() else {
                    return Command::none();
                };
                if f.scheme() != "file" {
                    return Command::none();
                }
                let Ok(path) = f.to_file_path() else {
                    return Command::none();
                };
                Command::perform(
                    async move { tokio::fs::read_to_string(path).await },
                    |res| {
                        if let Some(b) = res.ok().and_then(|s| ron::de::from_str(&s).ok()) {
                            crate::Message::PageMessage(crate::pages::Message::Appearance(
                                Message::ImportSuccess(Box::new(b)),
                            ))
                        } else {
                            // TODO Error toast?
                            tracing::error!("failed to import a file for a custom theme.");
                            crate::Message::PageMessage(crate::pages::Message::Appearance(
                                Message::ImportError,
                            ))
                        }
                    },
                )
            }
            Message::ExportFile(f) => {
                let Some(f) = f.uris().first() else {
                    return Command::none();
                };
                if f.scheme() != "file" {
                    return Command::none();
                }
                let Ok(path) = f.to_file_path() else {
                    return Command::none();
                };
                let Ok(builder) =
                    ron::ser::to_string_pretty(&self.theme_builder, PrettyConfig::default())
                else {
                    return Command::none();
                };
                Command::perform(
                    async move { tokio::fs::write(path, builder).await },
                    |res| {
                        if res.is_ok() {
                            crate::Message::PageMessage(crate::pages::Message::Appearance(
                                Message::ExportSuccess,
                            ))
                        } else {
                            // TODO Error toast?
                            tracing::error!(
                                "failed to select a file for importing a custom theme."
                            );
                            crate::Message::PageMessage(crate::pages::Message::Appearance(
                                Message::ExportError,
                            ))
                        }
                    },
                )
            }
            // TODO: error message toast?
            Message::ExportError | Message::ImportError => Command::none(),
            Message::ExportSuccess => {
                tracing::trace!("Export successful");
                Command::none()
            }
            Message::ImportSuccess(builder) => {
                tracing::trace!("Import successful");
                self.theme_builder = *builder;

                if let Some(config) = self.theme_builder_config.as_ref() {
                    _ = self.theme_builder.write_entry(config);
                };

                let config = if self.theme_mode.is_dark {
                    Theme::dark_config()
                } else {
                    Theme::light_config()
                };
                let new_theme = self.theme_builder.clone().build();
                if let Ok(config) = config {
                    _ = new_theme.write_entry(&config);
                } else {
                    tracing::error!("Failed to get the theme config.");
                }

                self.reload_theme_mode();
                Command::none()
            }
            Message::UseDefaultWindowHint(v) => {
                self.no_custom_window_hint = v;
                self.theme_builder_needs_update = true;
                let theme = if self.theme_mode.is_dark {
                    Theme::dark_default()
                } else {
                    Theme::light_default()
                };
                if !v {
                    let window_hint = self
                        .theme_builder
                        .window_hint
                        .filter(|c| {
                            let c = Srgba::new(c.red, c.green, c.blue, 1.0);
                            c != theme.palette.accent_blue
                                && c != theme.palette.accent_green
                                && c != theme.palette.accent_indigo
                                && c != theme.palette.accent_orange
                                && c != theme.palette.accent_pink
                                && c != theme.palette.accent_purple
                                && c != theme.palette.accent_red
                                && c != theme.palette.accent_warm_grey
                                && c != theme.palette.accent_yellow
                        })
                        .unwrap_or(
                            self.custom_accent
                                .get_applied_color()
                                .unwrap_or_default()
                                .into(),
                        );
                    _ = self.accent_window_hint.update::<app::Message>(
                        ColorPickerUpdate::ActiveColor(Hsv::from_color(window_hint)),
                    );
                };
                Command::none()
            }
            Message::ApplyThemeGlobal(enabled) => {
                if let Some(tk_config) = self.tk_config.as_ref() {
                    _ = self.tk.set_apply_theme_global(tk_config, enabled);
                } else {
                    tracing::error!("Failed to apply theme to GNOME config because the CosmicTK config does not exist.");
                }
                Command::none()
            }
            Message::ExperimentalContextDrawer => {
                self.context_view = Some(ContextView::Experimental);
                cosmic::command::message(crate::app::Message::OpenContextDrawer("".into()))
            }
            Message::Daytime(day_time) => {
                self.day_time = day_time;
                Command::none()
            }
        };

        if self.theme_builder_needs_update {
            let Some(config) = self.theme_builder_config.as_ref() else {
                return ret;
            };
            let mut theme_builder = std::mem::take(&mut self.theme_builder);
            theme_builder.bg_color = self
                .application_background
                .get_applied_color()
                .map(Srgba::from);
            theme_builder.primary_container_bg = self
                .container_background
                .get_applied_color()
                .map(Srgba::from);
            theme_builder.text_tint = self.interface_text.get_applied_color().map(Srgb::from);
            theme_builder.neutral_tint = self.control_component.get_applied_color().map(Srgb::from);
            theme_builder.window_hint = if self.no_custom_window_hint {
                None
            } else {
                self.accent_window_hint.get_applied_color().map(Srgb::from)
            };

            _ = theme_builder.write_entry(config);

            self.theme_builder = theme_builder;

            let config = if self.theme_mode.is_dark {
                Theme::dark_config()
            } else {
                Theme::light_config()
            };
            if let Ok(config) = config {
                let new_theme = self.theme_builder.clone().build();
                _ = new_theme.write_entry(&config);
            } else {
                tracing::error!("Failed to get the theme config.");
            }
        }

        self.can_reset = if self.theme_mode.is_dark {
            self.theme_builder != ThemeBuilder::dark()
        } else {
            self.theme_builder != ThemeBuilder::light()
        };

        if needs_sync {
            if let Err(err) = self.sync_changes() {
                tracing::error!(?err, "Error syncing theme changes.");
            }
        }

        ret
    }

    fn reload_theme_mode(&mut self) {
        let icon_themes = std::mem::take(&mut self.icon_themes);
        let icon_handles = std::mem::take(&mut self.icon_handles);
        let icon_theme_active = self.icon_theme_active.take();
        let day_time = self.day_time;

        *self = Self::from((self.theme_mode_config.clone(), self.theme_mode));
        self.day_time = day_time;
        self.icon_themes = icon_themes;
        self.icon_handles = icon_handles;
        self.icon_theme_active = icon_theme_active;
    }

    fn update_color_picker(
        &mut self,
        message: &ColorPickerUpdate,
        context_view: ContextView,
        context_title: Cow<'static, str>,
    ) -> Command<app::Message> {
        match message {
            ColorPickerUpdate::AppliedColor | ColorPickerUpdate::Reset => {
                self.theme_builder_needs_update = true;
                cosmic::command::message(crate::app::Message::CloseContextDrawer)
            }

            ColorPickerUpdate::ActionFinished => {
                self.theme_builder_needs_update = true;
                Command::none()
            }

            ColorPickerUpdate::Cancel => {
                cosmic::command::message(crate::app::Message::CloseContextDrawer)
            }

            ColorPickerUpdate::ToggleColorPicker => {
                self.context_view = Some(context_view);
                cosmic::command::message(crate::app::Message::OpenContextDrawer(context_title))
            }

            _ => Command::none(),
        }
    }

    fn update_panel_radii(roundness: Roundness) {
        let panel_config_helper = CosmicPanelConfig::cosmic_config("Panel").ok();
        let dock_config_helper = CosmicPanelConfig::cosmic_config("Dock").ok();
        let mut panel_config = panel_config_helper.as_ref().and_then(|config_helper| {
            let panel_config = CosmicPanelConfig::get_entry(config_helper).ok()?;
            (panel_config.name == "Panel").then_some(panel_config)
        });
        let mut dock_config = dock_config_helper.as_ref().and_then(|config_helper| {
            let panel_config = CosmicPanelConfig::get_entry(config_helper).ok()?;
            (panel_config.name == "Dock").then_some(panel_config)
        });

        if let Some(panel_config_helper) = panel_config_helper.as_ref() {
            if let Some(panel_config) = panel_config.as_mut() {
                let radii = if panel_config.anchor_gap || !panel_config.expand_to_edges {
                    let cornder_radii: CornerRadii = roundness.into();
                    cornder_radii.radius_xl[0] as u32
                } else {
                    0
                };
                let update = panel_config.set_border_radius(panel_config_helper, radii);
                if let Err(err) = update {
                    tracing::error!(?err, "Error updating panel corner radii");
                }
            }
        };

        if let Some(dock_config_helper) = dock_config_helper.as_ref() {
            if let Some(dock_config) = dock_config.as_mut() {
                let radii = if dock_config.anchor_gap || !dock_config.expand_to_edges {
                    let cornder_radii: CornerRadii = roundness.into();
                    cornder_radii.radius_xl[0] as u32
                } else {
                    0
                };
                let update = dock_config.set_border_radius(dock_config_helper, radii);
                if let Err(err) = update {
                    tracing::error!(?err, "Error updating dock corner radii");
                }
            }
        };
    }
}

impl page::Page<crate::pages::Message> for Page {
    fn content(
        &self,
        sections: &mut SlotMap<section::Entity, Section<crate::pages::Message>>,
    ) -> Option<page::Content> {
        Some(vec![
            sections.insert(mode_and_colors()),
            sections.insert(style()),
            sections.insert(window_management()),
            sections.insert(experimental()),
            sections.insert(reset_button()),
        ])
    }

    fn header_view(&self) -> Option<Element<'_, crate::pages::Message>> {
        let content = row::with_capacity(2)
            .spacing(self.theme_builder.spacing.space_xxs)
            .push(button::standard(fl!("import")).on_press(Message::StartImport))
            .push(button::standard(fl!("export")).on_press(Message::StartExport))
            .apply(container)
            .width(Length::Fill)
            .align_x(alignment::Horizontal::Right)
            .apply(Element::from)
            .map(crate::pages::Message::Appearance);

        Some(content)
    }

    fn info(&self) -> page::Info {
        page::Info::new("appearance", "preferences-appearance-symbolic")
            .title(fl!("appearance"))
            .description(fl!("appearance", "desc"))
    }

    fn on_enter(
        &mut self,
        _: page::Entity,
        _sender: tokio::sync::mpsc::Sender<crate::pages::Message>,
    ) -> Command<crate::pages::Message> {
        command::future(fetch_icon_themes()).map(crate::pages::Message::Appearance)
    }

    fn on_leave(&mut self) -> Command<crate::pages::Message> {
        command::message(crate::pages::Message::Appearance(Message::Left))
    }

    fn context_drawer(&self) -> Option<Element<'_, crate::pages::Message>> {
        let view = match self.context_view? {
            ContextView::AccentWindowHint => self.color_picker_context_view(
                None,
                RESET_TO_DEFAULT.as_str().into(),
                Message::AccentWindowHint,
                |this| &this.accent_window_hint,
            ),

            ContextView::ApplicationBackground => self.color_picker_context_view(
                None,
                RESET_TO_DEFAULT.as_str().into(),
                Message::ApplicationBackground,
                |this| &this.application_background,
            ),

            ContextView::ContainerBackground => self.color_picker_context_view(
                Some(fl!("container-background", "desc-detail").into()),
                fl!("container-background", "reset").into(),
                Message::ContainerBackground,
                |this| &this.container_background,
            ),

            ContextView::ControlComponent => self.color_picker_context_view(
                None,
                RESET_TO_DEFAULT.as_str().into(),
                Message::ControlComponent,
                |this| &this.control_component,
            ),

            ContextView::CustomAccent => self.color_picker_context_view(
                None,
                RESET_TO_DEFAULT.as_str().into(),
                Message::CustomAccent,
                |this| &this.custom_accent,
            ),

            ContextView::Experimental => self.experimental_context_view(),

            ContextView::InterfaceText => self.color_picker_context_view(
                None,
                RESET_TO_DEFAULT.as_str().into(),
                Message::InterfaceText,
                |this| &this.interface_text,
            ),
        };

        Some(view)
    }
}

#[allow(clippy::too_many_lines)]
pub fn mode_and_colors() -> Section<crate::pages::Message> {
    let mut descriptions = Slab::new();

    let auto_switch = descriptions.insert(fl!("auto-switch"));
    let accent_color = descriptions.insert(fl!("accent-color"));
    let app_bg = descriptions.insert(fl!("app-background"));
    let container_bg = descriptions.insert(fl!("container-background"));
    let container_bg_desc = descriptions.insert(fl!("container-background", "desc"));
    let text_tint = descriptions.insert(fl!("text-tint"));
    let text_tint_desc = descriptions.insert(fl!("text-tint", "desc"));
    let control_tint = descriptions.insert(fl!("control-tint"));
    let control_tint_desc = descriptions.insert(fl!("control-tint", "desc"));
    let window_hint_toggle = descriptions.insert(fl!("window-hint-accent-toggle"));
    let window_hint = descriptions.insert(fl!("window-hint-accent"));
    let dark = descriptions.insert(fl!("dark"));
    let light = descriptions.insert(fl!("light"));

    Section::default()
        .title(fl!("mode-and-colors"))
        .descriptions(descriptions)
        .view::<Page>(move |_binder, page, section| {
            let descriptions = &section.descriptions;
            let palette = &page.theme_builder.palette.as_ref();
            let cur_accent = page
                .theme_builder
                .accent
                .map_or(palette.accent_blue, Srgba::from);
            let mut section = settings::view_section(&section.title)
                .add(
                    container(
                        cosmic::iced::widget::row![
                            cosmic::iced::widget::column![
                                button(
                                    icon(from_name("illustration-appearance-mode-dark").into(),)
                                        .width(Length::Fill)
                                        .height(Length::Fixed(100.0))
                                )
                                .style(button::Style::Image)
                                .padding([8, 0])
                                .selected(page.theme_mode.is_dark)
                                .on_press(Message::DarkMode(true)),
                                text::body(&descriptions[dark])
                            ]
                            .spacing(8)
                            .width(Length::FillPortion(1))
                            .align_items(cosmic::iced_core::Alignment::Center),
                            cosmic::iced::widget::column![
                                button(
                                    icon(from_name("illustration-appearance-mode-light").into(),)
                                        .width(Length::Fill)
                                        .height(Length::Fixed(100.0))
                                )
                                .style(button::Style::Image)
                                .selected(!page.theme_mode.is_dark)
                                .padding([8, 0])
                                .on_press(Message::DarkMode(false)),
                                text::body(&descriptions[light])
                            ]
                            .spacing(8)
                            .width(Length::FillPortion(1))
                            .align_items(cosmic::iced_core::Alignment::Center)
                        ]
                        .spacing(48)
                        .align_items(cosmic::iced_core::Alignment::Center)
                        .width(Length::Fixed(424.0)),
                    )
                    .width(Length::Fill)
                    .align_x(cosmic::iced_core::alignment::Horizontal::Center),
                )
                .add(
                    settings::item::builder(&descriptions[auto_switch])
                        .description(
                            if !page.day_time && page.theme_mode.is_dark {
                                &page.auto_switch_descs[0]
                            } else if page.day_time && !page.theme_mode.is_dark {
                                &page.auto_switch_descs[1]
                            } else if page.day_time && page.theme_mode.is_dark {
                                &page.auto_switch_descs[2]
                            } else {
                                &page.auto_switch_descs[3]
                            }
                            .clone(),
                        )
                        .toggler(page.theme_mode.auto_switch, Message::Autoswitch),
                )
                .add(
                    cosmic::iced::widget::column![
                        text::body(&descriptions[accent_color]),
                        scrollable(
                            cosmic::iced::widget::row![
                                color_button(
                                    Some(Message::PaletteAccent(palette.accent_blue.into())),
                                    palette.accent_blue.into(),
                                    cur_accent == palette.accent_blue,
                                    48,
                                    48
                                ),
                                color_button(
                                    Some(Message::PaletteAccent(palette.accent_indigo.into())),
                                    palette.accent_indigo.into(),
                                    cur_accent == palette.accent_indigo,
                                    48,
                                    48
                                ),
                                color_button(
                                    Some(Message::PaletteAccent(palette.accent_purple.into())),
                                    palette.accent_purple.into(),
                                    cur_accent == palette.accent_purple,
                                    48,
                                    48
                                ),
                                color_button(
                                    Some(Message::PaletteAccent(palette.accent_pink.into())),
                                    palette.accent_pink.into(),
                                    cur_accent == palette.accent_pink,
                                    48,
                                    48
                                ),
                                color_button(
                                    Some(Message::PaletteAccent(palette.accent_red.into())),
                                    palette.accent_red.into(),
                                    cur_accent == palette.accent_red,
                                    48,
                                    48
                                ),
                                color_button(
                                    Some(Message::PaletteAccent(palette.accent_orange.into())),
                                    palette.accent_orange.into(),
                                    cur_accent == palette.accent_orange,
                                    48,
                                    48
                                ),
                                color_button(
                                    Some(Message::PaletteAccent(palette.accent_yellow.into())),
                                    palette.accent_yellow.into(),
                                    cur_accent == palette.accent_yellow,
                                    48,
                                    48
                                ),
                                color_button(
                                    Some(Message::PaletteAccent(palette.accent_green.into())),
                                    palette.accent_green.into(),
                                    cur_accent == palette.accent_green,
                                    48,
                                    48
                                ),
                                color_button(
                                    Some(Message::PaletteAccent(palette.accent_warm_grey.into())),
                                    palette.accent_warm_grey.into(),
                                    cur_accent == palette.accent_warm_grey,
                                    48,
                                    48
                                ),
                                if let Some(c) = page.custom_accent.get_applied_color() {
                                    container(color_button(
                                        Some(Message::CustomAccent(
                                            ColorPickerUpdate::ToggleColorPicker,
                                        )),
                                        c,
                                        cosmic::iced::Color::from(cur_accent) == c,
                                        48,
                                        48,
                                    ))
                                } else {
                                    container(
                                        page.custom_accent
                                            .picker_button(Message::CustomAccent, None)
                                            .width(Length::Fixed(48.0))
                                            .height(Length::Fixed(48.0)),
                                    )
                                },
                            ]
                            .padding([0, 0, 16, 0])
                            .spacing(16)
                        )
                        .direction(scrollable::Direction::Horizontal(
                            scrollable::Properties::new()
                        ))
                    ]
                    .padding([16, 24, 0, 24])
                    .spacing(8),
                )
                .add(
                    settings::item::builder(&descriptions[app_bg]).control(
                        page.application_background
                            .picker_button(Message::ApplicationBackground, Some(24))
                            .width(Length::Fixed(48.0))
                            .height(Length::Fixed(24.0)),
                    ),
                )
                .add(
                    settings::item::builder(&descriptions[container_bg])
                        .description(&descriptions[container_bg_desc])
                        .control(if page.container_background.get_applied_color().is_some() {
                            Element::from(
                                page.container_background
                                    .picker_button(Message::ContainerBackground, Some(24))
                                    .width(Length::Fixed(48.0))
                                    .height(Length::Fixed(24.0)),
                            )
                        } else {
                            container(
                                button::text(fl!("auto"))
                                    .trailing_icon(from_name("go-next-symbolic"))
                                    .on_press(Message::ContainerBackground(
                                        ColorPickerUpdate::ToggleColorPicker,
                                    )),
                            )
                            .into()
                        }),
                )
                .add(
                    settings::item::builder(&descriptions[text_tint])
                        .description(&descriptions[text_tint_desc])
                        .control(
                            page.interface_text
                                .picker_button(Message::InterfaceText, Some(24))
                                .width(Length::Fixed(48.0))
                                .height(Length::Fixed(24.0)),
                        ),
                )
                .add(
                    settings::item::builder(&descriptions[control_tint])
                        .description(&descriptions[control_tint_desc])
                        .control(
                            page.control_component
                                .picker_button(Message::ControlComponent, Some(24))
                                .width(Length::Fixed(48.0))
                                .height(Length::Fixed(24.0)),
                        ),
                )
                .add(
                    settings::item::builder(&descriptions[window_hint_toggle])
                        .toggler(page.no_custom_window_hint, Message::UseDefaultWindowHint),
                );
            if !page.no_custom_window_hint {
                section = section.add(
                    settings::item::builder(&descriptions[window_hint]).control(
                        page.accent_window_hint
                            .picker_button(Message::AccentWindowHint, Some(24))
                            .width(Length::Fixed(48.0))
                            .height(Length::Fixed(24.0)),
                    ),
                );
            }
            section
                .apply(Element::from)
                .map(crate::pages::Message::Appearance)
        })
}

#[allow(clippy::too_many_lines)]
pub fn style() -> Section<crate::pages::Message> {
    let mut descriptions = Slab::new();

    let round = descriptions.insert(fl!("style", "round"));
    let slightly_round = descriptions.insert(fl!("style", "slightly-round"));
    let square = descriptions.insert(fl!("style", "square"));

    Section::default()
        .title(fl!("style"))
        .descriptions(descriptions)
        .view::<Page>(move |_binder, page, section| {
            let descriptions = &section.descriptions;

            settings::view_section(&section.title)
                .add(
                    container(
                        cosmic::iced::widget::row![
                            cosmic::iced::widget::column![
                                button(
                                    icon(
                                        from_name(if page.theme_mode.is_dark {
                                            "illustration-appearance-dark-style-round"
                                        } else {
                                            "illustration-appearance-light-style-round"
                                        })
                                        .into()
                                    )
                                    .width(Length::Fill)
                                    .height(Length::Fixed(100.0))
                                )
                                .selected(matches!(page.roundness, Roundness::Round))
                                .style(button::Style::Image)
                                .padding(8)
                                .on_press(Message::Roundness(Roundness::Round)),
                                text::body(&descriptions[round])
                            ]
                            .spacing(8)
                            .width(Length::FillPortion(1))
                            .align_items(cosmic::iced_core::Alignment::Center),
                            cosmic::iced::widget::column![
                                button(
                                    icon(
                                        from_name(if page.theme_mode.is_dark {
                                            "illustration-appearance-dark-style-slightly-round"
                                        } else {
                                            "illustration-appearance-light-style-slightly-round"
                                        })
                                        .into()
                                    )
                                    .width(Length::Fill)
                                    .height(Length::Fixed(100.0))
                                )
                                .selected(matches!(page.roundness, Roundness::SlightlyRound))
                                .style(button::Style::Image)
                                .padding(8)
                                .on_press(Message::Roundness(Roundness::SlightlyRound)),
                                text::body(&descriptions[slightly_round])
                            ]
                            .spacing(8)
                            .width(Length::FillPortion(1))
                            .align_items(cosmic::iced_core::Alignment::Center),
                            cosmic::iced::widget::column![
                                button(
                                    icon(
                                        from_name(if page.theme_mode.is_dark {
                                            "illustration-appearance-dark-style-square"
                                        } else {
                                            "illustration-appearance-light-style-square"
                                        })
                                        .into(),
                                    )
                                    .width(Length::Fill)
                                    .height(Length::Fixed(100.0))
                                )
                                .width(Length::FillPortion(1))
                                .selected(matches!(page.roundness, Roundness::Square))
                                .style(button::Style::Image)
                                .padding(8)
                                .on_press(Message::Roundness(Roundness::Square)),
                                text::body(&descriptions[square])
                            ]
                            .spacing(8)
                            .align_items(cosmic::iced_core::Alignment::Center)
                            .width(Length::FillPortion(1))
                        ]
                        .spacing(12)
                        .width(Length::Fixed(628.0))
                        .align_items(cosmic::iced_core::Alignment::Center),
                    )
                    .width(Length::Fill)
                    .align_x(cosmic::iced_core::alignment::Horizontal::Center),
                )
                .apply(Element::from)
                .map(crate::pages::Message::Appearance)
        })
}

#[allow(clippy::too_many_lines)]
pub fn window_management() -> Section<crate::pages::Message> {
    let mut descriptions = Slab::new();

    let active_hint = descriptions.insert(fl!("window-management-appearance", "active-hint"));
    let gaps = descriptions.insert(fl!("window-management-appearance", "gaps"));

    Section::default()
        .title(fl!("window-management-appearance"))
        .descriptions(descriptions)
        .view::<Page>(move |_binder, page, section| {
            let descriptions = &section.descriptions;

            settings::view_section(&section.title)
                .add(settings::item::builder(&descriptions[active_hint]).control(
                    cosmic::widget::spin_button(
                        page.theme_builder.active_hint.to_string(),
                        Message::WindowHintSize,
                    ),
                ))
                .add(settings::item::builder(&descriptions[gaps]).control(
                    cosmic::widget::spin_button(
                        page.theme_builder.gaps.1.to_string(),
                        Message::GapSize,
                    ),
                ))
                .apply(Element::from)
                .map(crate::pages::Message::Appearance)
        })
}

pub fn experimental() -> Section<crate::pages::Message> {
    let mut descriptions = Slab::new();

    let experimental_label = descriptions.insert(fl!("experimental-settings"));

    Section::default()
        .descriptions(descriptions)
        .view::<Page>(move |_binder, _page, section| {
            let descriptions = &section.descriptions;

            let control = row::with_children(vec![
                horizontal_space(Length::Fill).into(),
                icon::from_name("go-next-symbolic").size(16).into(),
            ]);

            settings::view_section("")
                .add(
                    settings::item::builder(&descriptions[experimental_label])
                        .control(control)
                        .apply(container)
                        .style(cosmic::theme::Container::List)
                        .apply(button)
                        .style(cosmic::theme::Button::Transparent)
                        .on_press(Message::ExperimentalContextDrawer),
                )
                .apply(Element::from)
                .map(crate::pages::Message::Appearance)
        })
}

#[allow(clippy::too_many_lines)]
pub fn reset_button() -> Section<crate::pages::Message> {
    let mut descriptions = Slab::new();

    let reset_to_default = descriptions.insert(fl!("reset-to-default"));

    Section::default()
        .descriptions(descriptions)
        .view::<Page>(move |_binder, page, section| {
            let descriptions = &section.descriptions;
            if page.can_reset {
                button::standard(&descriptions[reset_to_default])
                    .on_press(Message::Reset)
                    .into()
            } else {
                horizontal_space(1).apply(Element::from)
            }
            .map(crate::pages::Message::Appearance)
        })
}
impl page::AutoBind<crate::pages::Message> for Page {}

/// A button for selecting a color or gradient.
pub fn color_button<'a, Message: 'a + Clone>(
    on_press: Option<Message>,
    color: cosmic::iced::Color,
    selected: bool,
    width: u16,
    height: u16,
) -> Element<'a, Message> {
    button(color_image(
        wallpaper::Color::Single([color.r, color.g, color.b]),
        width,
        height,
        None,
    ))
    .padding(0)
    .selected(selected)
    .style(button::Style::Image)
    .on_press_maybe(on_press)
    .width(Length::Fixed(f32::from(width)))
    .height(Length::Fixed(f32::from(height)))
    .into()
}

/// Find all icon themes available on the system.
async fn fetch_icon_themes() -> Message {
    let mut icon_themes = BTreeMap::new();
    let mut theme_paths: BTreeMap<String, PathBuf> = BTreeMap::new();

    let mut buffer = String::new();

    let xdg_data_home = std::env::var("XDG_DATA_HOME")
        .ok()
        .and_then(|value| {
            if value.is_empty() {
                None
            } else {
                Some(PathBuf::from(value))
            }
        })
        .or_else(dirs::home_dir)
        .map(|dir| dir.join(".local/share/icons"));

    let xdg_data_dirs = std::env::var("XDG_DATA_DIRS").ok();

    let xdg_data_dirs = xdg_data_dirs
        .as_deref()
        // Default from the XDG Base Directory Specification
        .or(Some("/usr/local/share/:/usr/share/"))
        .into_iter()
        .flat_map(|arg| std::env::split_paths(arg).map(|dir| dir.join("icons")));

    for icon_dir in xdg_data_dirs.chain(xdg_data_home) {
        let Ok(read_dir) = std::fs::read_dir(&icon_dir) else {
            continue;
        };

        'icon_dir: for entry in read_dir.filter_map(Result::ok) {
            let Ok(path) = entry.path().canonicalize() else {
                continue;
            };

            let Some(id) = entry.file_name().to_str().map(String::from) else {
                continue;
            };

            let manifest = path.join("index.theme");

            if !manifest.exists() {
                continue;
            }

            let Ok(file) = tokio::fs::File::open(&manifest).await else {
                continue;
            };

            buffer.clear();
            let mut name = None;
            let mut valid_dirs = Vec::new();

            let mut line_reader = tokio::io::BufReader::new(file);
            while let Ok(read) = line_reader.read_line(&mut buffer).await {
                if read == 0 {
                    break;
                }

                if let Some(is_hidden) = buffer.strip_prefix("Hidden=") {
                    if is_hidden.trim() == "true" {
                        continue 'icon_dir;
                    }
                } else if name.is_none() {
                    if let Some(value) = buffer.strip_prefix("Name=") {
                        name = Some(value.trim().to_owned());
                    }
                }

                if valid_dirs.is_empty() {
                    if let Some(value) = buffer.strip_prefix("Inherits=") {
                        valid_dirs.extend(value.trim().split(',').map(|fallback| {
                            if let Some(path) = theme_paths.get(fallback) {
                                path.iter()
                                    .last()
                                    .and_then(|os| os.to_str().map(ToOwned::to_owned))
                                    .unwrap_or_else(|| fallback.to_owned())
                            } else {
                                fallback.to_owned()
                            }
                        }));
                    }
                }

                buffer.clear();
            }

            if let Some(name) = name {
                // Name of the directory theme was found in (e.g. Pop for Pop)
                valid_dirs.push(
                    path.iter()
                        .last()
                        .and_then(|os| os.to_str().map(ToOwned::to_owned))
                        .unwrap_or_else(|| name.clone()),
                );
                theme_paths.entry(name.clone()).or_insert(path);

                let theme = id.clone();
                // `icon::from_name` may perform blocking I/O
                if let Ok(handles) =
                    tokio::task::spawn_blocking(|| preview_handles(theme, valid_dirs)).await
                {
                    icon_themes.insert(IconTheme { id, name }, handles);
                }
            }
        }
    }

    Message::Entered(icon_themes.into_iter().unzip())
}

/// Set the preferred icon theme for GNOME/GTK applications.
async fn set_gnome_icon_theme(theme: String) {
    let _res = tokio::process::Command::new("gsettings")
        .args([
            "set",
            "org.gnome.desktop.interface",
            "icon-theme",
            theme.as_str(),
        ])
        .status()
        .await;
}

/// Generate [icon::Handle]s to use for icon theme previews.
fn preview_handles(theme: String, inherits: Vec<String>) -> [icon::Handle; ICON_PREV_N] {
    // Cache current default and set icon theme as a temporary default
    let default = cosmic::icon_theme::default();
    cosmic::icon_theme::set_default(theme);

    // Evaluate handles with the temporary theme
    let handles = [
        icon_handle("folder", "folder-symbolic", &inherits),
        icon_handle("user-home", "user-home-symbolic", &inherits),
        icon_handle("text-x-generic", "text-x-generic-symbolic", &inherits),
        icon_handle("image-x-generic", "images-x-generic-symbolic", &inherits),
        icon_handle("audio-x-generic", "audio-x-generic-symbolic", &inherits),
        icon_handle("video-x-generic", "video-x-generic-symbolic", &inherits),
    ];

    // Reset default icon theme.
    cosmic::icon_theme::set_default(default);
    handles
}

/// Evaluate an icon handle for a specific theme.
///
/// `alternate` is a fallback icon name such as a symbolic variant.
///
/// `valid_dirs` should be a slice of directories from which we consider an icon to be valid. Valid
/// directories would usually be inherited themes as well as the actual theme's location.
fn icon_handle(icon_name: &str, alternate: &str, valid_dirs: &[String]) -> icon::Handle {
    ICON_TRY_SIZES
        .iter()
        .zip(std::iter::repeat(icon_name).take(ICON_TRY_SIZES.len()))
        // Try fallback icon name after the default
        .chain(
            ICON_TRY_SIZES
                .iter()
                .zip(std::iter::repeat(alternate))
                .take(ICON_TRY_SIZES.len()),
        )
        .find_map(|(&size, name)| {
            icon::from_name(name)
                // Set the size on the handle to evaluate the correct icon
                .size(size)
                // Get the path to the icon for the currently set theme.
                // Without the exact path, the handles will all resolve to icons from the same theme in
                // [`icon_theme_button`] rather than the icons for each different theme
                .path()
                // `libcosmic` should always return a path if the default theme is installed
                // The returned path has to be verified as an icon from the set theme or an
                // inherited theme
                .and_then(|path| {
                    let mut theme_dir = &*path;
                    while let Some(parent) = theme_dir.parent() {
                        if parent.ends_with("icons") {
                            break;
                        }
                        theme_dir = parent;
                    }

                    if let Some(dir_name) =
                        theme_dir.iter().last().and_then(std::ffi::OsStr::to_str)
                    {
                        valid_dirs
                            .iter()
                            .any(|valid| dir_name == valid)
                            .then(|| icon::from_path(path))
                    } else {
                        None
                    }
                })
        })
        // Fallback icon handle
        .unwrap_or_else(|| icon::from_name(icon_name).size(ICON_THUMB_SIZE).handle())
}

/// Button with a preview of the icon theme.
fn icon_theme_button(
    name: &str,
    handles: &[icon::Handle],
    id: usize,
    selected: bool,
) -> Element<'static, Message> {
    let theme = cosmic::theme::active();
    let theme = theme.cosmic();
    let background = Background::Color(theme.palette.neutral_4.into());

    cosmic::widget::column()
        .push(
            cosmic::widget::button::custom_image_button(
                cosmic::widget::column::with_children(vec![
                    cosmic::widget::row()
                        .extend(
                            handles
                                .iter()
                                .take(ICON_PREV_ROW)
                                .cloned()
                                // TODO: Maybe allow choosable sizes/zooming
                                .map(|handle| handle.icon().size(ICON_THUMB_SIZE)),
                        )
                        .spacing(theme.space_xxxs())
                        .into(),
                    cosmic::widget::row()
                        .extend(
                            handles
                                .iter()
                                .skip(ICON_PREV_ROW)
                                .cloned()
                                // TODO: Maybe allow choosable sizes/zooming
                                .map(|handle| handle.icon().size(ICON_THUMB_SIZE)),
                        )
                        .spacing(theme.space_xxxs())
                        .into(),
                ])
                .spacing(theme.space_xxxs()),
                None,
            )
            .on_press(Message::IconTheme(id))
            .selected(selected)
            .padding([theme.space_xs(), theme.space_xs() + 1])
            // Image button's style mostly works, but it needs a background to fit the design
            .style(button::Style::Custom {
                active: Box::new(move |focused, theme| {
                    let mut appearance = <cosmic::theme::Theme as button::StyleSheet>::active(
                        theme,
                        focused,
                        selected,
                        &cosmic::theme::Button::Image,
                    );
                    appearance.background = Some(background);
                    appearance
                }),
                disabled: Box::new(move |theme| {
                    let mut appearance = <cosmic::theme::Theme as button::StyleSheet>::disabled(
                        theme,
                        &cosmic::theme::Button::Image,
                    );
                    appearance.background = Some(background);
                    appearance
                }),
                hovered: Box::new(move |focused, theme| {
                    let mut appearance = <cosmic::theme::Theme as button::StyleSheet>::hovered(
                        theme,
                        focused,
                        selected,
                        &cosmic::theme::Button::Image,
                    );
                    appearance.background = Some(background);
                    appearance
                }),
                pressed: Box::new(move |focused, theme| {
                    let mut appearance = <cosmic::theme::Theme as button::StyleSheet>::pressed(
                        theme,
                        focused,
                        selected,
                        &cosmic::theme::Button::Image,
                    );
                    appearance.background = Some(background);
                    appearance
                }),
            }),
        )
        .push(
            text::body(if name.len() > ICON_NAME_TRUNC {
                format!("{name:.ICON_NAME_TRUNC$}...")
            } else {
                name.into()
            })
            .width(Length::Fixed((ICON_THUMB_SIZE * 3) as _)),
        )
        .spacing(theme.space_xxs())
        .into()
}
