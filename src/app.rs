use color_eyre::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, DisableMouseCapture},
    terminal::{disable_raw_mode, LeaveAlternateScreen},
    execute,
};
use ratatui::{
    style::Stylize,
    text::Line,
    widgets::{Block, Borders, Paragraph},
    Frame,
    prelude::*,
};
use std::{process::Command, path::Path, fs, path::PathBuf, panic};

pub enum AppState {
    CheckingDependencies,
    EnteringProjectName,
    ConfirmOverwrite,
    Installing(InstallStep),
    Success,
    Finished,
}

pub enum InstallStep {
    CloningRepo,
    SettingUpSparse,
    MovingFiles,
    UpdatingDependencies,
    SettingUpForge,
}

pub struct App {
    state: AppState,
    project_name: String,
    status_message: String,
    rust_installed: bool,
    foundry_installed: bool,
    risc0_version: Option<String>,
}

impl App {
    pub fn new() -> Self {
        // Set up panic hook to restore terminal on crash
        panic::set_hook(Box::new(|panic_info| {
            let _ = disable_raw_mode();
            let _ = execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
            eprintln!("Panic occurred: {:?}", panic_info);
        }));

        Self {
            state: AppState::CheckingDependencies,
            project_name: String::new(),
            status_message: String::from("Checking dependencies..."),
            rust_installed: false,
            foundry_installed: false,
            risc0_version: None,
        }
    }

    fn check_dependency(&mut self, cmd: &str, args: &[&str], success_msg: &str, error_msg: &str) -> bool {
        match Command::new(cmd).args(args).output() {
            Ok(_) => {
                self.status_message = format!("✓ {}", success_msg);
                true
            }
            Err(_) => {
                self.status_message = format!("✗ {}", error_msg);
                false
            }
        }
    }

    fn check_rust(&mut self) -> bool {
        self.check_dependency(
            "rustc",
            &["--version"],
            "Rust is installed",
            "Rust not found. Visit: https://www.rust-lang.org/tools/install"
        )
    }

    fn check_foundry(&mut self) -> bool {
        self.check_dependency(
            "forge",
            &["--version"],
            "Foundry is installed",
            "Foundry not found. Visit: https://book.getfoundry.sh/getting-started/installation"
        )
    }

    fn check_risc0(&mut self) -> bool {
        let output = Command::new("cargo")
            .arg("risczero")
            .arg("--version")
            .output();
        
        match output {
            Ok(output) => {
                if let Ok(version) = String::from_utf8(output.stdout) {
                    // Extract version number using regex or string operations
                    if version.contains("1.2.") {
                        self.risc0_version = Some(version.trim().to_string());
                        self.status_message = String::from("✓ RISC0 1.2.x detected");
                        return true;
                    }
                }
                self.status_message = String::from("✗ Unsupported RISC0 version. Version 1.2 is required");
                false
            }
            Err(_) => {
                self.status_message = String::from("✗ RISC0 not found. Visit: https://dev.risczero.com/api/zkvm/install");
                false
            }
        }
    }

    fn clone_repository(&self) -> Result<()> {
        // First check if directory already exists
        if Path::new(&self.project_name).exists() {
            return Err(color_eyre::eyre::eyre!("Directory '{}' already exists", self.project_name));
        }

        let output = Command::new("git")
            .args([
                "clone",
                "-b", "release-1.3",
                "https://github.com/risc0/risc0-ethereum.git",
                &self.project_name,
                "--single-branch",
                "--depth", "1"
            ])
            .output()?;

        if !output.status.success() {
            // Get the error message from stderr
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(color_eyre::eyre::eyre!("Failed to clone repository: {}", error));
        }
        Ok(())
    }

    fn setup_sparse_checkout(&self) -> Result<()> {
        // Change to project directory
        std::env::set_current_dir(&self.project_name)?;
        
        let output = Command::new("git")
            .args(["sparse-checkout", "set", "examples/erc20-counter"])
            .output()?;

        if !output.status.success() {
            return Err(color_eyre::eyre::eyre!("Failed to set sparse checkout"));
        }

        let output = Command::new("git")
            .arg("checkout")
            .output()?;

        if !output.status.success() {
            return Err(color_eyre::eyre::eyre!("Failed to checkout"));
        }

        // Verify the directory exists before proceeding
        if !Path::new("examples/erc20-counter").exists() {
            return Err(color_eyre::eyre::eyre!("examples/erc20-counter directory not found after checkout"));
        }

        Ok(())
    }

    fn move_files(&mut self) -> Result<()> {
        self.status_message = String::from("Moving erc20-counter out of examples/...");
        
        // First check if directories exist
        if !Path::new("examples").exists() || !Path::new("examples/erc20-counter").exists() {
            return Err(color_eyre::eyre::eyre!("Required directories not found. Expected examples/erc20-counter"));
        }

        // Create a temporary directory for the move
        let temp_dir = "temp_erc20";
        fs::create_dir(temp_dir)?;

        // First move everything to temp directory
        for entry in fs::read_dir("examples/erc20-counter")? {
            let entry = entry?;
            let path = entry.path();
            let file_name = path.file_name().unwrap();
            fs::rename(&path, Path::new(temp_dir).join(file_name))?;
        }

        // Clean up examples directory
        fs::remove_dir_all("examples")?;

        // Now move everything from temp to current directory
        self.status_message = String::from("Moving contents to root directory...");
        for entry in fs::read_dir(temp_dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_name = path.file_name().unwrap();
            fs::rename(&path, file_name)?;
        }

        // Clean up temp directory
        self.status_message = String::from("Cleaning up...");
        fs::remove_dir(temp_dir)?;
        
        Ok(())
    }

    fn update_dependencies(&mut self) -> Result<()> {
        self.status_message = String::from("Updating Cargo.toml files...");
        
        let cargo_files = self.find_cargo_toml_files(".")?;
        
        for file_path in cargo_files {
            self.status_message = format!("Updating {}...", file_path.display());
            
            let content = fs::read_to_string(&file_path)?;
            let is_apps = file_path.to_string_lossy().contains("/apps/");
            
            let updated_content = content
                .replace(
                    "risc0-build-ethereum = .*",
                    "risc0-build-ethereum = { git = \"https://github.com/risc0/risc0-ethereum\", branch = \"release-1.3\" }"
                )
                .replace(
                    "risc0-ethereum-contracts = .*",
                    "risc0-ethereum-contracts = { git = \"https://github.com/risc0/risc0-ethereum\", branch = \"release-1.3\" }"
                );

            // Add host feature for apps
            let updated_content = if is_apps {
                updated_content.replace(
                    "risc0-steel = .*",
                    "risc0-steel = { git = \"https://github.com/risc0/risc0-ethereum\", branch = \"release-1.3\", features = [\"host\"] }"
                )
            } else {
                updated_content.replace(
                    "risc0-steel = .*",
                    "risc0-steel = { git = \"https://github.com/risc0/risc0-ethereum\", branch = \"release-1.3\" }"
                )
            };
            
            fs::write(file_path, updated_content)?;
        }
        
        Ok(())
    }

    fn setup_forge(&mut self) -> Result<()> {
        self.status_message = String::from("Initializing git repository...");
        
        // Remove existing git directory and init new one
        let _ = fs::remove_dir_all(".git"); // Ignore error if doesn't exist
        Command::new("git")
            .arg("init")
            .output()?;

        // Get submodule commits from original repo
        let output = Command::new("git")
            .args(["submodule", "status"])
            .output()?;
        
        let stdout = String::from_utf8_lossy(&output.stdout);
        let forge_std_commit = stdout
            .lines()
            .find(|line| line.contains("forge-std"))
            .and_then(|line| line.split_whitespace().next())
            .map(|hash| hash.trim_start_matches(|c| c == '+' || c == '-'))
            .unwrap_or("");
        
        let oz_commit = stdout
            .lines()
            .find(|line| line.contains("openzeppelin-contracts"))
            .and_then(|line| line.split_whitespace().next())
            .map(|hash| hash.trim_start_matches(|c| c == '+' || c == '-'))
            .unwrap_or("");

        self.status_message = String::from("Creating lib directory...");
        fs::create_dir_all("lib")?;

        self.status_message = String::from("Adding forge-std submodule...");
        Command::new("git")
            .args([
                "submodule",
                "add",
                "https://github.com/foundry-rs/forge-std",
                "lib/forge-std"
            ])
            .output()?;
        
        if !forge_std_commit.is_empty() {
            Command::new("git")
                .args(["-C", "lib/forge-std", "checkout", forge_std_commit])
                .output()?;
        }

        self.status_message = String::from("Adding OpenZeppelin submodule...");
        Command::new("git")
            .args([
                "submodule",
                "add",
                "https://github.com/OpenZeppelin/openzeppelin-contracts",
                "lib/openzeppelin-contracts"
            ])
            .output()?;

        if !oz_commit.is_empty() {
            Command::new("git")
                .args(["-C", "lib/openzeppelin-contracts", "checkout", oz_commit])
                .output()?;
        }

        // Add risc0-ethereum submodule
        self.status_message = String::from("Adding risc0-ethereum submodule...");
        Command::new("git")
            .args([
                "submodule",
                "add",
                "-b", "release-1.3",
                "https://github.com/risc0/risc0-ethereum",
                "lib/risc0-ethereum"
            ])
            .output()?;

        // Update all submodules recursively
        self.status_message = String::from("Updating submodules recursively...");
        Command::new("git")
            .args([
                "submodule",
                "update",
                "--init",
                "--recursive",
                "--quiet"
            ])
            .output()?;

        // Clear the staged index
        Command::new("git")
            .args(["reset"])
            .output()?;

        self.status_message = String::from("Updating remappings...");
        let remappings = "\
            forge-std/=lib/forge-std/src/\n\
            openzeppelin/=lib/openzeppelin-contracts/\n\
            risc0/=lib/risc0-ethereum/contracts/src/\n\
            openzeppelin-contracts/=lib/openzeppelin-contracts/contracts\n";
        
        fs::write("remappings.txt", remappings)?;

        // Update foundry.toml
        if Path::new("foundry.toml").exists() {
            let content = fs::read_to_string("foundry.toml")?;
            let updated_content = content
                .replace(
                    "libs = [\"../../lib\", \"../../contracts/src\"]",
                    "libs = [\"lib\"]"
                );
            // Add auto_detect_remappings = false after [profile.default]
            let updated_content = if updated_content.contains("[profile.default]") {
                updated_content.replace(
                    "[profile.default]",
                    "[profile.default]\nauto_detect_remappings = false"
                )
            } else {
                updated_content + "\n[profile.default]\nauto_detect_remappings = false"
            };
            fs::write("foundry.toml", updated_content)?;
            self.status_message = String::from("Updated foundry.toml");
        }

        Ok(())
    }

    fn find_cargo_toml_files(&self, dir: &str) -> Result<Vec<PathBuf>> {
        let mut cargo_files = Vec::new();
        
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            
            if path.is_dir() {
                cargo_files.extend(self.find_cargo_toml_files(path.to_str().unwrap())?);
            } else if path.file_name().unwrap() == "Cargo.toml" {
                cargo_files.push(path);
            }
        }
        
        Ok(cargo_files)
    }

    fn handle_error(&mut self) -> Result<()> {
        self.status_message.push_str("\nPress Esc to exit");
        loop {
            if event::poll(std::time::Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.code == KeyCode::Esc {
                        disable_raw_mode()?;
                        execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
                        eprintln!("Error: {}", self.status_message);
                        std::process::exit(1);
                    }
                }
            }
        }
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> Result<bool> {
        if key.kind != KeyEventKind::Press {
            return Ok(false);
        }

        match (&self.state, key.code) {
            (_, KeyCode::Esc) => return Ok(true),
            
            (AppState::EnteringProjectName, KeyCode::Enter) => {
                if !self.project_name.is_empty() {
                    if Path::new(&self.project_name).exists() {
                        self.state = AppState::ConfirmOverwrite;
                        self.status_message = format!(
                            "Directory '{}' already exists. Overwrite? (y/N)",
                            self.project_name
                        );
                    } else {
                        self.state = AppState::Installing(InstallStep::CloningRepo);
                        self.status_message = format!("Installing project '{}'...", self.project_name);
                    }
                }
            }
            
            (AppState::EnteringProjectName, KeyCode::Char(c)) => {
                self.project_name.push(c);
            }
            
            (AppState::EnteringProjectName, KeyCode::Backspace) => {
                self.project_name.pop();
            }
            
            (AppState::ConfirmOverwrite, KeyCode::Char('y' | 'Y')) => {
                if let Err(e) = fs::remove_dir_all(&self.project_name) {
                    self.status_message = format!("Error removing directory: {}", e);
                    self.handle_error()?;
                } else {
                    self.state = AppState::Installing(InstallStep::CloningRepo);
                    self.status_message = format!("Installing project '{}'...", self.project_name);
                }
            }
            
            (AppState::ConfirmOverwrite, KeyCode::Char('n' | 'N') | KeyCode::Enter) => {
                self.state = AppState::EnteringProjectName;
                self.project_name.clear();
                self.status_message = String::from("Enter project name (press Enter when done):");
            }
            
            _ => {}
        }

        Ok(false)
    }

    pub fn run(&mut self, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        loop {
            terminal.draw(|frame| self.ui(frame))?;
            
            if event::poll(std::time::Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if self.handle_key_event(key)? {
                        return Ok(());
                    }
                }
            }

            match &self.state {
                AppState::CheckingDependencies => {
                    if !self.rust_installed {
                        self.rust_installed = self.check_rust();
                    }
                    if !self.foundry_installed {
                        self.foundry_installed = self.check_foundry();
                    }
                    if self.risc0_version.is_none() {
                        self.check_risc0();
                    }

                    if self.rust_installed && self.foundry_installed && self.risc0_version.is_some() {
                        self.state = AppState::EnteringProjectName;
                        self.status_message = String::from("Enter project name (press Enter when done):");
                    }
                }
                AppState::Installing(step) => {
                    let result = match step {
                        InstallStep::CloningRepo => {
                            match self.clone_repository() {
                                Ok(_) => {
                                    self.state = AppState::Installing(InstallStep::SettingUpSparse);
                                    Ok(())
                                }
                                Err(e) => Err(e),
                            }
                        }
                        InstallStep::SettingUpSparse => {
                            match self.setup_sparse_checkout() {
                                Ok(_) => {
                                    self.state = AppState::Installing(InstallStep::MovingFiles);
                                    Ok(())
                                }
                                Err(e) => Err(e),
                            }
                        }
                        InstallStep::MovingFiles => {
                            match self.move_files() {
                                Ok(_) => {
                                    self.state = AppState::Installing(InstallStep::UpdatingDependencies);
                                    Ok(())
                                }
                                Err(e) => Err(e),
                            }
                        }
                        InstallStep::UpdatingDependencies => {
                            match self.update_dependencies() {
                                Ok(_) => {
                                    self.state = AppState::Installing(InstallStep::SettingUpForge);
                                    Ok(())
                                }
                                Err(e) => Err(e),
                            }
                        }
                        InstallStep::SettingUpForge => {
                            match self.setup_forge() {
                                Ok(_) => {
                                    self.state = AppState::Success;
                                    self.status_message = format!("✓ Project '{}' created successfully!", self.project_name);
                                    Ok(())
                                }
                                Err(e) => Err(e),
                            }
                        }
                    };

                    if let Err(e) = result {
                        self.status_message = format!("Error: {}", e);
                        self.handle_error()?;
                    }
                }
                AppState::Success => {
                    if event::poll(std::time::Duration::from_millis(50))? {
                        if let Event::Key(_) = event::read()? {
                            self.state = AppState::Finished;
                        }
                    }
                }
                AppState::Finished => break,
                _ => {}
            }
        }
        Ok(())
    }

    /// Renders the user interface.
    ///
    /// This is where you add new widgets. See the following resources for more information:
    /// - <https://docs.rs/ratatui/latest/ratatui/widgets/index.html>
    /// - <https://github.com/ratatui/ratatui/tree/master/examples>
    fn ui(&self, frame: &mut Frame) {
        let area = frame.area();
        
        let main_block = Block::default()
            .title("Steel App Creator")
            .borders(Borders::ALL);
        
        let inner_area = main_block.inner(area);
        frame.render_widget(main_block, area);

        // Create a layout for status message and input
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Length(1), // Status message
                Constraint::Length(1), // Input field
                Constraint::Length(3), // Dependency status
                Constraint::Min(0),    // Remaining space
            ])
            .split(inner_area);

        // Render status message
        let status = Paragraph::new(self.status_message.clone());
        frame.render_widget(status, chunks[0]);

        // Render input field when in EnteringProjectName state
        if let AppState::EnteringProjectName = self.state {
            let input = Paragraph::new(self.project_name.clone())
                .block(Block::default()
                    .borders(Borders::NONE)
                    .style(Style::default().fg(Color::Yellow)));
            frame.render_widget(input, chunks[1]);
        }

        // Show dependency status
        if let AppState::CheckingDependencies = self.state {
            let deps_status = vec![
                format!("Rust: {}", if self.rust_installed { "✓" } else { "..." }),
                format!("Foundry: {}", if self.foundry_installed { "✓" } else { "..." }),
                format!("RISC0: {}", self.risc0_version.as_ref().map_or("...", |v| v)),
            ].join("\n");
            
            let deps = Paragraph::new(deps_status)
                .style(Style::default().fg(Color::Gray));
            frame.render_widget(deps, chunks[2]);
        }

        // Show installation progress when installing
        if let AppState::Installing(step) = &self.state {
            let (progress, details) = match step {
                InstallStep::CloningRepo => (
                    "Step 1/5: Cloning Repository",
                    format!("• Cloning risc0-ethereum into '{}'\n• Branch: release-1.3", self.project_name)
                ),
                InstallStep::SettingUpSparse => (
                    "Step 2/5: Setting up Sparse Checkout",
                    "• Configuring sparse checkout\n• Selecting erc20-counter template".to_string()
                ),
                InstallStep::MovingFiles => (
                    "Step 3/5: Moving Files",
                    "• Moving erc20-counter out of examples\n• Reorganizing project structure".to_string()
                ),
                InstallStep::UpdatingDependencies => (
                    "Step 4/5: Updating Dependencies",
                    "• Updating Cargo.toml files\n• Configuring git dependencies".to_string()
                ),
                InstallStep::SettingUpForge => (
                    "Step 5/5: Setting up Forge",
                    "• Initializing git repository\n• Setting up forge-std and OpenZeppelin\n• Configuring remappings".to_string()
                ),
            };
            
            let progress_text = vec![
                Line::from(progress).style(Style::default().fg(Color::Blue).bold()),
                Line::from(""),
                Line::from(details),
            ];
            
            let progress_widget = Paragraph::new(progress_text)
                .block(Block::default().borders(Borders::NONE));
            frame.render_widget(progress_widget, chunks[2]);
        }

        // Add confirmation dialog display
        if let AppState::ConfirmOverwrite = self.state {
            let confirm_text = vec![
                Line::from("Directory already exists!").style(Style::default().fg(Color::Yellow)),
                Line::from(""),
                Line::from("Press 'y' to overwrite"),
                Line::from("Press 'n' or Enter to choose a different name"),
                Line::from("Press Esc to exit"),
            ];
            
            let confirm = Paragraph::new(confirm_text)
                .block(Block::default().borders(Borders::NONE));
            frame.render_widget(confirm, chunks[2]);
        }

        // Add success message display
        if let AppState::Success = self.state {
            let success_text = vec![
                Line::from("✨ Success! ✨").style(Style::default().fg(Color::Green).bold()),
                Line::from(""),
                Line::from(format!("Project '{}' has been created successfully!", self.project_name)),
                Line::from(""),
                Line::from("Next steps:"),
                Line::from(format!("  cd {}", self.project_name)),
                Line::from("  forge build"),
                Line::from("  forge test"),
                Line::from(""),
                Line::from("Press any key to exit...").style(Style::default().fg(Color::Gray)),
            ];
            
            let success = Paragraph::new(success_text)
                .block(Block::default().borders(Borders::NONE));
            frame.render_widget(success, chunks[2]);
        }
    }
}
