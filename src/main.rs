use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::prelude::*;
use std::io::stdout;

pub use app::App;

pub mod app;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    
    // Setup terminal
    enable_raw_mode()?;
    stdout()
        .execute(EnterAlternateScreen)?
        .execute(EnableMouseCapture)?;
    
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    // Create and run app
    let result = App::new().run(&mut terminal);

    // Restore terminal
    disable_raw_mode()?;
    stdout()
        .execute(LeaveAlternateScreen)?
        .execute(DisableMouseCapture)?;
    
    result
}
