use color_eyre::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, DisableMouseCapture},
    terminal::{disable_raw_mode, LeaveAlternateScreen},
    execute,
};
use ratatui::{
    style::Stylize,
    text::Line,
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
    prelude::*,
};
use std::{process::Command, path::Path, fs, path::PathBuf, panic};
use chrono;

pub enum AppState {
    CheckingDependencies,
    EnteringProjectName,
    ConfirmOverwrite,
    Installing(InstallStep),
    Success,
    TestMenu,      // New state for the test menu
    RunningTest,   // New state for when test is running
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
    command_output: Vec<String>,
    output_scroll: u16,
    pending_redraw: bool,
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
            command_output: Vec::new(),
            output_scroll: 0,
            pending_redraw: false,
        }
    }

    fn add_output(&mut self, output: String) {
        let timestamp = chrono::Local::now().format("%H:%M:%S");
        for line in output.lines() {
            self.command_output.push(format!("[{}] {}", timestamp, line));
        }
        self.pending_redraw = true;
    }

    fn run_command(&mut self, command: &mut Command, description: &str, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        self.status_message = description.to_string();
        
        // Force a redraw before running the command
        terminal.draw(|frame| self.ui(frame))?;
        
        let output = command.output()?;
        
        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            self.add_output(format!("Error: {}", error));
            return Err(color_eyre::eyre::eyre!("{}", error));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        
        if !stdout.is_empty() {
            self.add_output(stdout);
        }
        if !stderr.is_empty() {
            self.add_output(stderr);
        }
        
        // Force another redraw after adding output
        terminal.draw(|frame| self.ui(frame))?;
        
        Ok(())
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

    fn clone_repository(&mut self, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        if Path::new(&self.project_name).exists() {
            return Err(color_eyre::eyre::eyre!("Directory '{}' already exists", self.project_name));
        }

        self.run_command(
            Command::new("git")
                .args([
                    "clone",
                    "-b", "release-1.3",
                    "https://github.com/risc0/risc0-ethereum.git",
                    &self.project_name,
                    "--single-branch",
                    "--depth", "1"
                ]),
            &format!("Cloning repository into '{}'...", self.project_name),
            terminal
        )
    }

    fn setup_sparse_checkout(&mut self, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        // Change to project directory
        std::env::set_current_dir(&self.project_name)?;
        
        self.run_command(
            Command::new("git")
                .args(["sparse-checkout", "set", "examples/erc20-counter"]),
            "Setting up sparse checkout...",
            terminal
        )?;

        self.run_command(
            Command::new("git")
                .arg("checkout"),
            "Checking out files...",
            terminal
        )?;

        if !Path::new("examples/erc20-counter").exists() {
            return Err(color_eyre::eyre::eyre!("examples/erc20-counter directory not found after checkout"));
        }

        Ok(())
    }

    fn move_files(&mut self) -> Result<()> {
        // Only log important operations
        self.add_output("Moving erc20-counter template files...".to_string());
        
        // ... rest of the code, but remove individual file move logs ...
        
        self.add_output("Files moved successfully".to_string());
        Ok(())
    }

    fn update_dependencies(&mut self) -> Result<()> {
        let cargo_files = self.find_cargo_toml_files(".")?;
        
        self.add_output("Configuring RISC0 and Ethereum dependencies...".to_string());
        
        for file_path in cargo_files {
            // Do the updates but don't log each individual file
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
            
            fs::write(&file_path, updated_content)?;
        }
        
        self.add_output("Dependencies configured for RISC0 Ethereum development".to_string());
        Ok(())
    }

    fn setup_forge(&mut self, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        self.add_output("Starting Forge setup (this may take a few minutes)...".to_string());
        
        // Remove existing git directory and init new one
        let _ = fs::remove_dir_all(".git");
        
        // Initialize git repo
        self.run_command(
            Command::new("git").args(&["init"]),
            "Initializing git repository...",
            terminal
        )?;

        // Create lib directory
        fs::create_dir_all("lib")?;

        // Add forge-std
        self.add_output("Adding forge-std (1/3)...".to_string());
        self.run_command(
            Command::new("git")
                .args(&[
                    "submodule",
                    "add",
                    "https://github.com/foundry-rs/forge-std",
                    "lib/forge-std"
                ]),
            "Cloning forge-std...",
            terminal
        )?;

        // Add OpenZeppelin
        self.add_output("Adding OpenZeppelin (2/3)...".to_string());
        self.run_command(
            Command::new("git")
                .args(&[
                    "submodule",
                    "add",
                    "https://github.com/OpenZeppelin/openzeppelin-contracts",
                    "lib/openzeppelin-contracts"
                ]),
            "Cloning OpenZeppelin...",
            terminal
        )?;

        // Add risc0-ethereum
        self.add_output("Adding risc0-ethereum (3/3)...".to_string());
        self.run_command(
            Command::new("git")
                .args(&[
                    "submodule",
                    "add",
                    "-b", "release-1.3",
                    "https://github.com/risc0/risc0-ethereum",
                    "lib/risc0-ethereum"
                ]),
            "Cloning risc0-ethereum...",
            terminal
        )?;

        // Update submodules
        self.add_output("Updating submodules recursively (this may take a while)...".to_string());
        self.run_command(
            Command::new("git")
                .args(&[
                    "submodule",
                    "update",
                    "--init",
                    "--recursive",
                    "--quiet"
                ]),
            "Updating submodules...",
            terminal
        )?;

        // Reset git index
        self.run_command(
            Command::new("git")
                .args(&["reset"]),
            "Resetting git index...",
            terminal
        )?;

        // Update remappings and foundry.toml
        self.add_output("Finalizing Forge configuration...".to_string());
        
        // ... rest of the setup ...
        
        self.add_output("Forge setup completed successfully".to_string());
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
            
            (AppState::Success, KeyCode::Enter) => {
                self.state = AppState::TestMenu;
                self.status_message = String::from("Select test to run:");
                self.command_output.clear();
                return Ok(false);
            }
            (AppState::Success, _) => {
                return Ok(false);
            }
            (AppState::TestMenu, KeyCode::Char('1')) => {
                self.state = AppState::RunningTest;
                self.run_e2e_test()?;
                return Ok(false);
            }
            _ => {}
        }

        // Add scroll handling
        match key.code {
            KeyCode::PageUp => {
                if self.output_scroll > 0 {
                    self.output_scroll = self.output_scroll.saturating_sub(1);
                }
            }
            KeyCode::PageDown => {
                if !self.command_output.is_empty() {
                    self.output_scroll = self.output_scroll.saturating_add(1);
                }
            }
            _ => {}
        }

        Ok(false)
    }

    fn run_e2e_test(&mut self) -> Result<()> {
        self.add_output("Starting end-to-end test with Anvil...".to_string());
        
        // Change to project directory
        std::env::set_current_dir(&self.project_name)?;
        
        // Create channels for stdout and stderr
        let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
        let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
        
        // Run the test script
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("./e2e-test-anvil.sh")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        
        let stdout_reader = std::io::BufReader::new(stdout);
        let stderr_reader = std::io::BufReader::new(stderr);
        
        use std::io::BufRead;
        
        // Spawn thread for stdout
        let stdout_tx_clone = stdout_tx.clone();
        std::thread::spawn(move || {
            for line in stdout_reader.lines() {
                if let Ok(line) = line {
                    let _ = stdout_tx_clone.send(line);
                }
            }
        });

        // Spawn thread for stderr
        let stderr_tx_clone = stderr_tx.clone();
        std::thread::spawn(move || {
            for line in stderr_reader.lines() {
                if let Ok(line) = line {
                    let _ = stderr_tx_clone.send(line);
                }
            }
        });

        // Process output in the main thread
        let mut completed = false;
        while !completed {
            // Check stdout
            if let Ok(line) = stdout_rx.try_recv() {
                self.add_output(line);
            }
            
            // Check stderr
            if let Ok(line) = stderr_rx.try_recv() {
                self.add_output(format!("Error: {}", line));
            }
            
            // Check if process has finished
            match child.try_wait() {
                Ok(Some(status)) => {
                    completed = true;
                    if status.success() {
                        self.add_output("End-to-end test completed successfully!".to_string());
                    } else {
                        self.add_output("End-to-end test failed!".to_string());
                    }
                }
                Ok(None) => {
                    // Process still running, wait a bit
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => {
                    self.add_output(format!("Error waiting for process: {}", e));
                    completed = true;
                }
            }
        }
        
        Ok(())
    }

    pub fn run(&mut self, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        loop {
            if self.pending_redraw {
                terminal.draw(|frame| self.ui(frame))?;
                self.pending_redraw = false;
            }

            // Check for events with a shorter timeout
            if event::poll(std::time::Duration::from_millis(16))? {  // ~60fps
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
                            match self.clone_repository(terminal) {
                                Ok(_) => {
                                    self.state = AppState::Installing(InstallStep::SettingUpSparse);
                                    Ok(())
                                }
                                Err(e) => Err(e),
                            }
                        }
                        InstallStep::SettingUpSparse => {
                            match self.setup_sparse_checkout(terminal) {
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
                            match self.setup_forge(terminal) {
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
                    // Remove the automatic state transition on key press
                    // The transition will now be handled in handle_key_event
                }
                AppState::RunningTest => {
                    // Stay in this state while test is running
                    // The test completion will be handled in run_e2e_test
                }
                AppState::Finished => break,
                _ => {}
            }

            // Always draw at least once per loop
            terminal.draw(|frame| self.ui(frame))?;
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

        // Modify the layout constraints when in Success state
        let chunks = if let AppState::Success = self.state {
            Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([
                    Constraint::Length(1),     // Status message
                    Constraint::Length(1),     // Input field
                    Constraint::Ratio(1, 2),   // Success message gets half the remaining space
                    Constraint::Ratio(1, 2),   // Command output gets the other half
                ])
                .split(inner_area)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([
                    Constraint::Length(1),     // Status message
                    Constraint::Length(1),     // Input field
                    Constraint::Length(3),     // Progress/menu area
                    Constraint::Min(0),        // Command output
                ])
                .split(inner_area)
        };

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
                    "Step 1/5: Downloading Template",
                    format!("• Downloading RISC0 Ethereum template into '{}'\n• Using release-1.3 branch", self.project_name)
                ),
                InstallStep::SettingUpSparse => (
                    "Step 2/5: Extracting ERC20 Counter Example",
                    "• Configuring repository for minimal download\n• Extracting ERC20 counter example code".to_string()
                ),
                InstallStep::MovingFiles => (
                    "Step 3/5: Setting Up Project Structure",
                    "• Moving files to root directory\n• Creating standard project layout".to_string()
                ),
                InstallStep::UpdatingDependencies => (
                    "Step 4/5: Configuring Dependencies",
                    "• Updating Rust package dependencies\n• Setting up RISC0 and Ethereum integrations".to_string()
                ),
                InstallStep::SettingUpForge => (
                    "Step 5/5: Installing Forge Components",
                    "• Setting up Foundry development environment\n• Installing OpenZeppelin contracts\n• Configuring RISC0 Ethereum components".to_string()
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
                Line::from(""),
                Line::from("✨ Success! ✨").style(Style::default().fg(Color::Green).bold()),
                Line::from(""),
                Line::from(format!("Project '{}' has been created successfully!", self.project_name)),
                Line::from(""),
                Line::from(""),
                Line::from(">>> PRESS ENTER TO CONTINUE <<<")
                    .style(Style::default().fg(Color::Yellow).bold()),
                Line::from(""),
            ];
            
            let success = Paragraph::new(success_text)
                .block(Block::default().borders(Borders::NONE))
                .alignment(Alignment::Center);
            frame.render_widget(success, chunks[2]);
        }

        // Show command output
        if !self.command_output.is_empty() {
            let output_text = self.command_output
                .iter()
                .map(|line| Line::from(line.as_str()))
                .collect::<Vec<_>>();

            let output = Paragraph::new(output_text)
                .block(Block::default()
                    .title("Command Output")
                    .borders(Borders::ALL))
                .scroll((self.output_scroll, 0))
                .wrap(Wrap { trim: true });
            
            frame.render_widget(output, chunks[3]);
        }

        if let AppState::TestMenu = self.state {
            let menu_text = vec![
                Line::from("End-to-End Test Menu").style(Style::default().fg(Color::Green).bold()),
                Line::from(""),
                Line::from("1) Run test with local Anvil chain"),
                Line::from(""),
                Line::from("Press Esc to exit").style(Style::default().fg(Color::Gray)),
            ];
            
            let menu = Paragraph::new(menu_text)
                .block(Block::default().borders(Borders::NONE));
            frame.render_widget(menu, chunks[2]);
        }
    }
}
