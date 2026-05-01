pub mod app;
pub mod audio;
pub mod storage;
pub mod ui;

use app::{FrameTiming, InputMode, Overlay};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use storage::config::KeyBindings;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let skip_menu = args.iter().any(|a| a == "--skip-menu" || a == "-s");

    // Parse boot speed: --boot-speed=fast|normal|slow|off (default: normal)
    let boot_speed = args
        .iter()
        .find(|a| a.starts_with("--boot-speed"))
        .and_then(|a| a.strip_prefix("--boot-speed="))
        .unwrap_or("normal");

    let speed = match boot_speed {
        "fast" => ui::launcher::BootSpeed::Fast,
        "slow" => ui::launcher::BootSpeed::Slow,
        "off" => ui::launcher::BootSpeed::Off,
        _ => ui::launcher::BootSpeed::Normal,
    };

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    // Show launch menu unless --skip-menu was passed
    if !skip_menu {
        match ui::launcher::show(&mut terminal, speed) {
            Ok(true) => {} // User chose "Start Radio"
            Ok(false) => {
                // User chose "Quit"
                disable_raw_mode()?;
                execute!(
                    terminal.backend_mut(),
                    LeaveAlternateScreen,
                    DisableMouseCapture
                )?;
                terminal.show_cursor()?;
                return Ok(());
            }
            Err(e) => {
                disable_raw_mode()?;
                execute!(
                    terminal.backend_mut(),
                    LeaveAlternateScreen,
                    DisableMouseCapture
                )?;
                terminal.show_cursor()?;
                return Err(e.into());
            }
        }
    }

    // Construct the app immediately with an empty station list.
    // The initial fetch runs in the background — stations appear once it completes.
    let mut app = app::App::new(Vec::new());
    // If visualizer is disabled, start in low-power mode
    if !app.visualizer_enabled {
        app.tick_rate_ms = 200;
    }
    app.start_initial_fetch();

    let mut last_tick = Instant::now();

    loop {
        let frame_start = Instant::now();

        // ── Draw ──────────────────────────────────────────────────
        let draw_start = Instant::now();
        terminal.draw(|f| ui::draw(f, &app))?;
        let draw_us = draw_start.elapsed().as_micros() as u64;

        // ── Event handling ────────────────────────────────────────
        let tick_rate = Duration::from_millis(app.tick_rate_ms);
        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        // Measure the idle wait separately from event handling work
        let wait_start = Instant::now();
        let has_event = crossterm::event::poll(timeout)?;
        let event_wait_us = wait_start.elapsed().as_micros() as u64;

        let handle_start = Instant::now();
        if has_event {
            if let Event::Key(key) = event::read()? {
                // On Windows, crossterm sends both Press and Release events.
                // Only act on Press to avoid double-firing every keystroke.
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match app.input_mode {
                    InputMode::Normal => {
                        // ── Theme picker overlay ──
                        if app.overlay == Overlay::ThemePicker {
                            let themes = crate::ui::themes::Theme::all();
                            let total = themes.len();
                            match key.code {
                                KeyCode::Esc => {
                                    app.overlay = Overlay::None;
                                }
                                _ if app.keybindings.theme_picker.matches(key.code) => {
                                    app.overlay = Overlay::None;
                                }
                                KeyCode::Up | KeyCode::Char('k') => {
                                    if app.theme_selected > 0 {
                                        app.theme_selected -= 1;
                                        // Live preview
                                        app.theme = themes.into_iter().nth(app.theme_selected).unwrap();
                                    }
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    if app.theme_selected < total - 1 {
                                        app.theme_selected += 1;
                                        // Live preview
                                        app.theme = themes.into_iter().nth(app.theme_selected).unwrap();
                                    }
                                }
                                KeyCode::Enter => {
                                    app.theme = themes.into_iter().nth(app.theme_selected).unwrap();
                                    app.save_config();
                                    app.overlay = Overlay::None;
                                }
                                _ => {}
                            }
                            continue;
                        }

                        // ── Genre picker overlay ──
                        if app.overlay == Overlay::GenrePicker {
                            match key.code {
                                KeyCode::Esc => {
                                    app.overlay = Overlay::None;
                                }
                                _ if app.keybindings.genre_picker.matches(key.code) => {
                                    app.overlay = Overlay::None;
                                }
                                KeyCode::Up | KeyCode::Char('k') => {
                                    if app.genre_selected > 0 {
                                        app.genre_selected -= 1;
                                    }
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    if app.genre_selected < app.categories.len() - 1 {
                                        app.genre_selected += 1;
                                    }
                                }
                                KeyCode::Enter => {
                                    app.select_genre(app.genre_selected);
                                    app.overlay = Overlay::None;
                                }
                                _ => {}
                            }
                            continue;
                        }

                        // ── Settings overlay has its own input handling ──
                        if app.overlay == Overlay::Settings {
                            // If awaiting a key for rebinding
                            if let Some((action_idx, is_alt)) = app.settings_awaiting_key {
                                match key.code {
                                    KeyCode::Esc => {
                                        // Cancel the rebind
                                        app.settings_awaiting_key = None;
                                    }
                                    new_key => {
                                        if let Some(json_key) = app.keybindings.key_at_index(action_idx) {
                                            let json_key = json_key.to_string();
                                            if is_alt {
                                                // Set alt, keep primary
                                                let actions = app.keybindings.all_actions();
                                                let primary = actions[action_idx].2.primary;
                                                app.keybindings.set_binding(&json_key, primary, Some(new_key));
                                            } else {
                                                // Set primary, keep alt
                                                let actions = app.keybindings.all_actions();
                                                let alt = actions[action_idx].2.alt;
                                                app.keybindings.set_binding(&json_key, new_key, alt);
                                            }
                                            app.save_config();
                                        }
                                        app.settings_awaiting_key = None;
                                    }
                                }
                            } else {
                                // Normal settings navigation
                                match key.code {
                                    KeyCode::Esc | KeyCode::Char('S') => {
                                        app.overlay = Overlay::None;
                                    }
                                    KeyCode::Up | KeyCode::Char('k') => {
                                        if app.settings_selected > 0 {
                                            app.settings_selected -= 1;
                                        }
                                    }
                                    KeyCode::Down | KeyCode::Char('j') => {
                                        let count = app.keybindings.all_actions().len();
                                        if app.settings_selected < count - 1 {
                                            app.settings_selected += 1;
                                        }
                                    }
                                    KeyCode::Enter => {
                                        // Start rebinding primary key
                                        app.settings_awaiting_key = Some((app.settings_selected, false));
                                    }
                                    KeyCode::Char('a') => {
                                        // Start rebinding alt key
                                        app.settings_awaiting_key = Some((app.settings_selected, true));
                                    }
                                    KeyCode::Char('d') => {
                                        // Clear the alt binding
                                        if let Some(json_key) = app.keybindings.key_at_index(app.settings_selected) {
                                            let json_key = json_key.to_string();
                                            let actions = app.keybindings.all_actions();
                                            let primary = actions[app.settings_selected].2.primary;
                                            app.keybindings.set_binding(&json_key, primary, None);
                                            app.save_config();
                                        }
                                    }
                                    KeyCode::Char('r') => {
                                        // Reset this action to default
                                        let defaults = KeyBindings::default();
                                        let default_actions = defaults.all_actions();
                                        if let Some((json_key, _, def_binding)) = default_actions.get(app.settings_selected) {
                                            let json_key = json_key.to_string();
                                            app.keybindings.set_binding(&json_key, def_binding.primary, def_binding.alt);
                                            app.save_config();
                                        }
                                    }
                                    KeyCode::Char('R') => {
                                        // Reset ALL to defaults
                                        app.keybindings = KeyBindings::default();
                                        app.save_config();
                                    }
                                    _ => {}
                                }
                            }
                            continue;
                        }

                        // Handle other overlays (help, detail)
                        if app.overlay != Overlay::None {
                            match key.code {
                                KeyCode::Esc => {
                                    app.overlay = Overlay::None;
                                }
                                _ if app.keybindings.help.matches(key.code) => {
                                    app.overlay = Overlay::None;
                                }
                                _ if app.keybindings.station_detail.matches(key.code) => {
                                    app.overlay = Overlay::None;
                                }
                                _ => {}
                            }
                            continue;
                        }

                        // ── Normal mode: use configured keybindings ──
                        let kc = key.code;

                        if app.keybindings.quit.matches(kc) {
                            break;
                        } else if app.keybindings.help.matches(kc) {
                            app.overlay = Overlay::Help;
                        } else if app.keybindings.station_detail.matches(kc) {
                            app.overlay = Overlay::StationDetail;
                        } else if app.keybindings.settings.matches(kc) {
                            app.overlay = Overlay::Settings;
                            app.settings_awaiting_key = None;
                        } else if app.keybindings.genre_picker.matches(kc) {
                            app.genre_selected = app.category_index;
                            app.overlay = Overlay::GenrePicker;
                        } else if app.keybindings.theme_picker.matches(kc) {
                            // Open theme picker overlay
                            let themes = crate::ui::themes::Theme::all();
                            app.theme_selected = themes.iter()
                                .position(|t| t.name == app.theme.name)
                                .unwrap_or(0);
                            app.overlay = Overlay::ThemePicker;
                        } else if app.keybindings.visualizer_toggle.matches(kc) {
                            app.visualizer_enabled = !app.visualizer_enabled;
                            app.player.visualizer_enabled = app.visualizer_enabled;
                            if !app.visualizer_enabled {
                                // Stop audio capture and drop to low-power tick rate
                                app.player.stop_capture_if_running();
                                app.tick_rate_ms = 200; // 5 FPS — plenty for static UI
                            } else {
                                // Restore user's configured tick rate from config
                                let config = crate::storage::config::Config::load();
                                app.tick_rate_ms = config.tick_rate_ms;
                                if app.player.is_playing() {
                                    app.player.restart_capture();
                                }
                            }
                            app.save_config();
                        } else if app.keybindings.search.matches(kc) {
                            app.search_query.clear();
                            app.input_mode = InputMode::Editing;
                        } else if app.keybindings.stop.matches(kc) {
                            app.stop();
                        } else if app.keybindings.toggle_favorite.matches(kc) {
                            app.toggle_favorite();
                        } else if app.keybindings.volume_up.matches(kc) {
                            app.set_volume(5);
                        } else if app.keybindings.volume_down.matches(kc) {
                            app.set_volume(-5);
                        } else if app.keybindings.navigate_down.matches(kc) {
                            app.next();
                        } else if app.keybindings.navigate_up.matches(kc) {
                            app.previous();
                        } else if app.keybindings.play.matches(kc) {
                            app.play();
                        } else if app.keybindings.cycle_panel.matches(kc) {
                            if key.modifiers.contains(KeyModifiers::SHIFT) {
                                app.switch_category();
                            } else {
                                app.cycle_panel();
                            }
                        } else if kc == KeyCode::BackTab {
                            app.switch_category_back();
                        } else if app.keybindings.genre_prev.matches(kc) {
                            app.switch_category_back();
                        } else if app.keybindings.genre_next.matches(kc) {
                            app.switch_category();
                        } else if app.keybindings.load_more.matches(kc) {
                            app.load_more();
                        } else if app.keybindings.perf_toggle.matches(kc) {
                            app.show_perf = !app.show_perf;
                        } else if app.show_perf && app.keybindings.perf_tick_slower.matches(kc) {
                            app.tick_rate_ms = (app.tick_rate_ms + 10).min(200);
                            app.save_config();
                        } else if app.show_perf && app.keybindings.perf_tick_faster.matches(kc) {
                            app.tick_rate_ms = app.tick_rate_ms.saturating_sub(10).max(10);
                            app.save_config();

                        // Smoothing adjustment (only when profiler is open)
                        } else if app.show_perf && kc == KeyCode::Char('{') {
                            // Decrease smoothing (more responsive)
                            let nr = app.visualizer.noise_reduction;
                            app.visualizer.noise_reduction = ((nr - 0.05) * 100.0).round() / 100.0;
                            if app.visualizer.noise_reduction < 0.05 {
                                app.visualizer.noise_reduction = 0.05;
                            }
                        } else if app.show_perf && kc == KeyCode::Char('}') {
                            // Increase smoothing (smoother)
                            let nr = app.visualizer.noise_reduction;
                            app.visualizer.noise_reduction = ((nr + 0.05) * 100.0).round() / 100.0;
                            if app.visualizer.noise_reduction > 0.95 {
                                app.visualizer.noise_reduction = 0.95;
                            }
                        }
                    }
                    InputMode::Editing => match key.code {
                        KeyCode::Enter => {
                            app.input_mode = InputMode::Normal;
                            app.perform_search();
                        }
                        KeyCode::Esc => {
                            app.input_mode = InputMode::Normal;
                        }
                        KeyCode::Char(c) => {
                            app.search_query.push(c);
                        }
                        KeyCode::Backspace => {
                            app.search_query.pop();
                        }
                        _ => {}
                    },
                }
            }
        }
        let event_handle_us = handle_start.elapsed().as_micros() as u64;

        // ── Tick: poll mpv IPC and update visualizer ──────────────
        let mut poll_us = 0u64;
        let mut vis_us = 0u64;
        let mut had_tick = false;

        if last_tick.elapsed() >= tick_rate {
            had_tick = true;
            let poll_start = Instant::now();
            app.player.poll();
            app.check_song_change();

            // Check if a background station fetch has completed
            app.poll_fetch();

            // Update FFT rate measurement for profiler
            app.update_fft_rate();

            poll_us = poll_start.elapsed().as_micros() as u64;

            let vis_start = Instant::now();
            if app.visualizer_enabled {
                if app.player.has_real_audio() {
                    let used_real = app.visualizer.tick_real(&app.analysis, app.volume);
                    if !used_real {
                        app.visualizer.tick_simulated(app.player.is_playing(), app.player.audio_level, app.volume);
                    }
                } else {
                    let level = app.player.audio_level;
                    app.visualizer.tick_simulated(app.player.is_playing(), level, app.volume);
                }
            }
            vis_us = vis_start.elapsed().as_micros() as u64;

            last_tick = Instant::now();
        }

        // ── Record frame timing ───────────────────────────────────
        let total_us = frame_start.elapsed().as_micros() as u64;
        let tick_budget_us = app.tick_rate_ms * 1000;
        app.perf.record(FrameTiming {
            draw_us,
            event_wait_us,
            event_handle_us,
            poll_us,
            vis_us,
            total_us,
            had_tick,
        }, tick_budget_us);
    }

    // Stop playback before the shutdown animation
    app.stop();

    // CRT power-off animation
    ui::shutdown::play(&mut terminal)?;

    // Cleanup
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}