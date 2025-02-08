use chrono;
use color_eyre::Result;
use crossterm::{
    event::{self, DisableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    style::Stylize,
    text::Line,
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use regex;
use std::{fs, panic, path::Path, path::PathBuf, process::Command};

#[derive(Default)]
pub enum AppState {
    #[default]
    CheckingDependencies,
    EnteringProjectName,
    ConfirmOverwrite,
    Installing(InstallStep),
    Success,
    TestMenu,
    Testing(E2ETestStep), // Changed to use new test step enum
    Finished,
}

pub enum InstallStep {
    CloningRepo,
    SettingUpSparse,
    MovingFiles,
    UpdatingDependencies,
    SettingUpForge,
}

#[derive(Clone)]
pub enum E2ETestStep {
    PreparingEnvironment, // Set up env vars
    StartingAnvil,        // Start anvil
    RunningTest,          // Run the actual script
    Cleanup,              // Clean up processes
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
    selected_menu_item: usize,
    confirm_menu_item: usize,
    anvil_process: Option<std::process::Child>,
    test_env: Option<TestEnvironment>, // Add this to store test-related data
}

struct TestEnvironment {
    eth_rpc_url: String,
    eth_wallet_address: String,
    eth_wallet_private_key: String,
    anvil_process: Option<std::process::Child>,
}

impl App {
    pub fn new() -> Self {
        // Set up panic hook to restore terminal on crash and kill anvil
        panic::set_hook(Box::new(|panic_info| {
            let _ = disable_raw_mode();
            let _ = execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
            // Try to kill anvil if it's running
            let _ = Command::new("pkill").arg("anvil").output();
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
            selected_menu_item: 0,
            confirm_menu_item: 0,
            anvil_process: None,
            test_env: None,
        }
    }

    fn add_output(&mut self, output: String) {
        let timestamp = chrono::Local::now().format("%H:%M:%S");
        for line in output.lines() {
            self.command_output
                .push(format!("[{}] {}", timestamp, line));
        }
        self.pending_redraw = true;
    }

    fn run_command(
        &mut self,
        command: &mut Command,
        description: &str,
        terminal: &mut Terminal<impl Backend>,
    ) -> Result<()> {
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

    fn check_dependency(
        &mut self,
        cmd: &str,
        args: &[&str],
        success_msg: &str,
        error_msg: &str,
    ) -> bool {
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
            "Rust not found. Visit: https://www.rust-lang.org/tools/install",
        )
    }

    fn check_foundry(&mut self) -> bool {
        self.check_dependency(
            "forge",
            &["--version"],
            "Foundry is installed",
            "Foundry not found. Visit: https://book.getfoundry.sh/getting-started/installation",
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
                self.status_message =
                    String::from("✗ Unsupported RISC0 version. Version 1.2 is required");
                false
            }
            Err(_) => {
                self.status_message = String::from(
                    "✗ RISC0 not found. Visit: https://dev.risczero.com/api/zkvm/install",
                );
                false
            }
        }
    }

    fn clone_repository(&mut self, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        // If directory exists, remove it first
        if Path::new(&self.project_name).exists() {
            self.add_output(format!(
                "Removing existing directory '{}'...",
                self.project_name
            ));
            fs::remove_dir_all(&self.project_name)?;
        }

        self.run_command(
            Command::new("git").args([
                "clone",
                "-b",
                "release-1.3",
                "https://github.com/risc0/risc0-ethereum.git",
                &self.project_name,
                "--single-branch",
                "--depth",
                "1",
            ]),
            &format!("Cloning repository into '{}'...", self.project_name),
            terminal,
        )
    }

    fn setup_sparse_checkout(&mut self, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        // Change to project directory
        std::env::set_current_dir(&self.project_name)?;

        self.run_command(
            Command::new("git").args(["sparse-checkout", "set", "examples/erc20-counter"]),
            "Setting up sparse checkout...",
            terminal,
        )?;

        self.run_command(
            Command::new("git").arg("checkout"),
            "Checking out files...",
            terminal,
        )?;

        if !Path::new("examples/erc20-counter").exists() {
            return Err(color_eyre::eyre::eyre!(
                "examples/erc20-counter directory not found after checkout"
            ));
        }

        Ok(())
    }

    fn move_files(&mut self) -> Result<()> {
        self.add_output("Moving template files to root directory...".to_string());

        // Move erc20-counter out of examples/
        fs::rename("examples/erc20-counter", "./erc20-counter")?;

        // Remove examples directory
        fs::remove_dir_all("examples")?;

        // Remove all files in root (but keep directories)
        for entry in fs::read_dir(".")? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                fs::remove_file(path)?;
            }
        }

        // Move all contents from erc20-counter to root (including hidden files)
        for entry in fs::read_dir("erc20-counter")? {
            let entry = entry?;
            let source = entry.path();
            let file_name = source.file_name().unwrap();
            let target = PathBuf::from(".").join(file_name);
            fs::rename(source, target)?;
        }

        // Remove the now-empty erc20-counter directory
        fs::remove_dir("erc20-counter")?;

        self.add_output("✓ Project structure set up successfully".to_string());
        Ok(())
    }

    fn update_dependencies(&mut self) -> Result<()> {
        let cargo_files = self.find_cargo_toml_files(".")?;

        self.add_output("Updating Cargo.toml files with git dependencies...".to_string());

        for file_path in cargo_files {
            let mut content = fs::read_to_string(&file_path)?;
            let is_apps = file_path.to_string_lossy().contains("/apps/");
            let is_workspace = content.contains("[workspace]");

            if is_workspace && content.contains("[workspace.dependencies]") {
                // For workspace manifests, do direct string replacements to match the expected format
                content = content
                    .replace(
                        "risc0-build-ethereum = { path = \"../../build\" }",
                        "risc0-build-ethereum = { git = \"https://github.com/risc0/risc0-ethereum\", branch = \"release-1.3\" }"
                    )
                    .replace(
                        "risc0-ethereum-contracts = { path = \"../../contracts\" }",
                        "risc0-ethereum-contracts = { git = \"https://github.com/risc0/risc0-ethereum\", branch = \"release-1.3\" }"
                    )
                    .replace(
                        "risc0-steel = { path = \"../../crates/steel\" }",
                        "risc0-steel = { git = \"https://github.com/risc0/risc0-ethereum\", branch = \"release-1.3\" }"
                    );
            } else if is_workspace {
                // Fallback: use regex with multi-line flag for workspace dependencies
                let re_ws_build = regex::Regex::new(
                    r#"(?m)^\s*risc0-build-ethereum\s*=\s*\{\s*path\s*=\s*".*"\s*\}"#,
                )
                .unwrap();
                let re_ws_contracts = regex::Regex::new(
                    r#"(?m)^\s*risc0-ethereum-contracts\s*=\s*\{\s*path\s*=\s*".*"\s*\}"#,
                )
                .unwrap();
                let re_ws_steel =
                    regex::Regex::new(r#"(?m)^\s*risc0-steel\s*=\s*\{\s*path\s*=\s*".*"\s*\}"#)
                        .unwrap();

                content = re_ws_build.replace_all(&content,
                    "risc0-build-ethereum = { git = \"https://github.com/risc0/risc0-ethereum\", branch = \"release-1.3\" }"
                ).to_string();
                content = re_ws_contracts.replace_all(&content,
                    "risc0-ethereum-contracts = { git = \"https://github.com/risc0/risc0-ethereum\", branch = \"release-1.3\" }"
                ).to_string();
                content = re_ws_steel.replace_all(&content,
                    "risc0-steel = { git = \"https://github.com/risc0/risc0-ethereum\", branch = \"release-1.3\" }"
                ).to_string();
            } else {
                // Handle regular dependencies using regex with multi-line flag
                let re_build = regex::Regex::new(r#"(?m)^risc0-build-ethereum\s*=.*$"#).unwrap();
                let re_contracts =
                    regex::Regex::new(r#"(?m)^risc0-ethereum-contracts\s*=.*$"#).unwrap();
                let re_steel = regex::Regex::new(r#"(?m)^risc0-steel\s*=.*$"#).unwrap();

                let risc0_build_ethereum = "risc0-build-ethereum = { git = \"https://github.com/risc0/risc0-ethereum\", branch = \"release-1.3\" }";
                let risc0_ethereum_contracts = "risc0-ethereum-contracts = { git = \"https://github.com/risc0/risc0-ethereum\", branch = \"release-1.3\" }";
                let risc0_steel = if is_apps {
                    "risc0-steel = { git = \"https://github.com/risc0/risc0-ethereum\", branch = \"release-1.3\", features = [\"host\"] }"
                } else {
                    "risc0-steel = { git = \"https://github.com/risc0/risc0-ethereum\", branch = \"release-1.3\" }"
                };

                content = re_build
                    .replace_all(&content, risc0_build_ethereum)
                    .to_string();
                content = re_contracts
                    .replace_all(&content, risc0_ethereum_contracts)
                    .to_string();
                content = re_steel.replace_all(&content, risc0_steel).to_string();
            }

            fs::write(&file_path, content)?;
            self.add_output(format!("Updated dependencies in: {}", file_path.display()));
        }

        self.add_output(
            "✓ All Cargo.toml files have been updated with git dependencies.".to_string(),
        );
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
            terminal,
        )?;

        // Create lib directory
        fs::create_dir_all("lib")?;

        // Add forge-std
        self.add_output("Adding forge-std (1/3)...".to_string());
        self.run_command(
            Command::new("git").args(&[
                "submodule",
                "add",
                "https://github.com/foundry-rs/forge-std",
                "lib/forge-std",
            ]),
            "Cloning forge-std...",
            terminal,
        )?;

        // Add OpenZeppelin
        self.add_output("Adding OpenZeppelin (2/3)...".to_string());
        self.run_command(
            Command::new("git").args(&[
                "submodule",
                "add",
                "https://github.com/OpenZeppelin/openzeppelin-contracts",
                "lib/openzeppelin-contracts",
            ]),
            "Cloning OpenZeppelin...",
            terminal,
        )?;

        // Add risc0-ethereum
        self.add_output("Adding risc0-ethereum (3/3)...".to_string());
        self.run_command(
            Command::new("git").args(&[
                "submodule",
                "add",
                "-b",
                "release-1.3",
                "https://github.com/risc0/risc0-ethereum",
                "lib/risc0-ethereum",
            ]),
            "Cloning risc0-ethereum...",
            terminal,
        )?;

        // Update submodules
        self.add_output("Updating submodules recursively (this may take a while)...".to_string());
        self.run_command(
            Command::new("git").args(&["submodule", "update", "--init", "--recursive", "--quiet"]),
            "Updating submodules...",
            terminal,
        )?;

        // Reset git index
        self.run_command(
            Command::new("git").args(&["reset"]),
            "Resetting git index...",
            terminal,
        )?;

        // Update remappings.txt
        if Path::new("remappings.txt").exists() {
            let mut content = fs::read_to_string("remappings.txt")?;

            // Update existing remappings
            content = content
                .replace(
                    "forge-std/=../../lib/forge-std/src/",
                    "forge-std/=lib/forge-std/src/",
                )
                .replace(
                    "openzeppelin/=../../lib/openzeppelin-contracts/",
                    "openzeppelin/=lib/openzeppelin-contracts/",
                )
                .replace(
                    "risc0/=../../contracts/src/",
                    "risc0/=lib/risc0-ethereum/contracts/src/",
                );

            // Add OpenZeppelin contracts remapping if not present
            if !content.contains("openzeppelin-contracts/=") {
                content.push_str("\nopenzeppelin-contracts/=lib/openzeppelin-contracts/contracts");
            }

            fs::write("remappings.txt", content)?;
            self.add_output("✓ Updated remappings.txt".to_string());
        } else {
            self.add_output("Warning: remappings.txt not found".to_string());
        }

        // Update foundry.toml
        if Path::new("foundry.toml").exists() {
            let mut content = fs::read_to_string("foundry.toml")?;

            // Update libs path
            content = content.replace(
                "libs = [\"../../lib\", \"../../contracts/src\"]",
                "libs = [\"lib\"]",
            );

            // Add auto_detect_remappings = false under [profile.default]
            if !content.contains("auto_detect_remappings") {
                if content.contains("[profile.default]") {
                    content = content.replace(
                        "[profile.default]",
                        "[profile.default]\nauto_detect_remappings = false",
                    );
                } else {
                    content.push_str("\n[profile.default]\nauto_detect_remappings = false");
                }
            }

            fs::write("foundry.toml", content)?;
            self.add_output("✓ Updated foundry.toml".to_string());
        } else {
            self.add_output("Warning: foundry.toml not found".to_string());
        }

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

    fn cleanup_test(&mut self) {
        if let Some(test_env) = &mut self.test_env {
            // Kill anvil process if it exists
            if let Some(mut child) = test_env.anvil_process.take() {
                let _ = child.kill();
            }
        }
        // Also try pkill just to be sure
        let _ = Command::new("pkill").arg("anvil").output();
    }

    fn handle_test_step(&mut self, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        let current_step = if let AppState::Testing(step) = &self.state {
            step.clone()
        } else {
            return Ok(());
        };

        let mut messages = Vec::new();

        let next_state = match current_step {
            E2ETestStep::PreparingEnvironment => {
                messages.push("Setting up test environment...".to_string());

                // Create test environment with standard Anvil test account
                self.test_env = Some(TestEnvironment {
                    eth_rpc_url: "http://localhost:8545".to_string(),
                    eth_wallet_address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".to_string(),
                    eth_wallet_private_key:
                        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
                            .to_string(),
                    anvil_process: None,
                });

                messages.push("✓ Environment variables configured".to_string());
                Some(AppState::Testing(E2ETestStep::StartingAnvil))
            }
            E2ETestStep::StartingAnvil => {
                messages.push("Starting local Ethereum chain...".to_string());

                // Kill any existing anvil process first
                let _ = Command::new("pkill").arg("anvil").output();

                // Start new anvil process with test mnemonic
                if let Some(test_env) = &mut self.test_env {
                    let child = Command::new("anvil")
                        .args([
                            "--mnemonic",
                            "test test test test test test test test test test test test junk",
                            "--silent",
                        ])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()?;

                    test_env.anvil_process = Some(child);

                    // Wait a moment for anvil to start
                    std::thread::sleep(std::time::Duration::from_secs(2));

                    messages.push("✓ Local Ethereum chain started".to_string());
                    Some(AppState::Testing(E2ETestStep::RunningTest))
                } else {
                    return Err(color_eyre::eyre::eyre!("Test environment not initialized"));
                }
            }
            E2ETestStep::RunningTest => {
                messages.push("Running end-to-end test...".to_string());

                // Debug: Print current directory
                if let Ok(current_dir) = std::env::current_dir() {
                    messages.push(format!("Current directory: {:?}", current_dir));
                }

                // First make sure we're in the workspace root
                let workspace_root = std::path::PathBuf::from("/Users/sasha/Developer/tui");
                std::env::set_current_dir(&workspace_root)?;

                // Then change to project directory
                messages.push(format!(
                    "Changing to project directory: {}",
                    self.project_name
                ));
                std::env::set_current_dir(&self.project_name)?;

                // Debug: Verify we're in the right place
                if let Ok(current_dir) = std::env::current_dir() {
                    messages.push(format!("New directory: {:?}", current_dir));
                }

                // List directory contents to debug
                if let Ok(entries) = std::fs::read_dir(".") {
                    let files: Vec<_> = entries
                        .filter_map(|e| e.ok())
                        .map(|e| e.file_name().to_string_lossy().to_string())
                        .collect();
                    messages.push(format!("Directory contents: {:?}", files));
                }

                // Set up environment variables
                if let Some(env) = &self.test_env {
                    std::env::set_var("ETH_RPC_URL", &env.eth_rpc_url);
                    std::env::set_var("ETH_WALLET_ADDRESS", &env.eth_wallet_address);
                    std::env::set_var("ETH_WALLET_PRIVATE_KEY", &env.eth_wallet_private_key);
                }

                // Check if the script exists
                if !std::path::Path::new("e2e-test.sh").exists() {
                    messages.push("Error: e2e-test.sh not found in current directory".to_string());
                    return Err(color_eyre::eyre::eyre!("e2e-test.sh script not found"));
                }

                // Run the script
                self.run_command(
                    Command::new("bash")
                        .arg("e2e-test.sh")
                        .env("RUST_LOG", "info,risc0_steel=debug"),
                    "Running end-to-end test script...",
                    terminal,
                )?;

                messages.push("✓ End-to-end test completed successfully".to_string());
                Some(AppState::Testing(E2ETestStep::Cleanup))
            }
            E2ETestStep::Cleanup => {
                messages.push("Cleaning up...".to_string());
                self.cleanup_test();
                messages.push("✓ Cleanup completed".to_string());
                Some(AppState::TestMenu)
            }
        };

        // Add all collected messages
        for msg in messages {
            self.add_output(msg);
        }

        // Update state if we got a new one
        if let Some(new_state) = next_state {
            self.state = new_state;
        }

        Ok(())
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
                        self.status_message = String::from(""); // Remove the duplicate message
                    } else {
                        self.state = AppState::Installing(InstallStep::CloningRepo);
                        self.status_message =
                            format!("Installing project '{}'...", self.project_name);
                    }
                }
            }

            (AppState::EnteringProjectName, KeyCode::Char(c)) => {
                self.project_name.push(c);
            }

            (AppState::EnteringProjectName, KeyCode::Backspace) => {
                self.project_name.pop();
            }

            (AppState::ConfirmOverwrite, KeyCode::Up) => {
                self.confirm_menu_item = self.confirm_menu_item.saturating_sub(1);
                return Ok(false);
            }

            (AppState::ConfirmOverwrite, KeyCode::Down) => {
                self.confirm_menu_item = (self.confirm_menu_item + 1).min(2); // 2 options (0, 1, 2)
                return Ok(false);
            }

            (AppState::ConfirmOverwrite, KeyCode::Enter) => {
                match self.confirm_menu_item {
                    0 => {
                        // Continue (overwrite)
                        self.state = AppState::Installing(InstallStep::CloningRepo);
                        self.status_message =
                            format!("Installing project '{}'...", self.project_name);
                    }
                    1 => {
                        // Go to testing toolbox
                        self.state = AppState::TestMenu;
                        self.status_message = String::from("Select test to run:");
                        self.command_output.clear();
                    }
                    2 => {
                        // Exit
                        return Ok(true);
                    }
                    _ => {}
                }
                return Ok(false);
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
            (AppState::TestMenu, KeyCode::Up) => {
                self.selected_menu_item = self.selected_menu_item.saturating_sub(1);
                return Ok(false);
            }
            (AppState::TestMenu, KeyCode::Down) => {
                self.selected_menu_item = (self.selected_menu_item + 1).min(1); // 1 is the last menu item
                return Ok(false);
            }
            (AppState::TestMenu, KeyCode::Enter) => {
                match self.selected_menu_item {
                    0 => {
                        self.state = AppState::Testing(E2ETestStep::PreparingEnvironment);
                        self.status_message = String::from("Running end-to-end test...");
                    }
                    1 => {
                        self.state = AppState::Finished;
                    }
                    _ => {}
                }
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

    pub fn run(&mut self, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        loop {
            if self.pending_redraw {
                terminal.draw(|frame| self.ui(frame))?;
                self.pending_redraw = false;
            }

            // Check for events with a shorter timeout
            if event::poll(std::time::Duration::from_millis(16))? {
                // ~60fps
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

                    if self.rust_installed && self.foundry_installed && self.risc0_version.is_some()
                    {
                        self.state = AppState::EnteringProjectName;
                        self.status_message =
                            String::from("Enter project name (press Enter when done):");
                    }
                }
                AppState::Installing(step) => {
                    let result = match step {
                        InstallStep::CloningRepo => match self.clone_repository(terminal) {
                            Ok(_) => {
                                self.state = AppState::Installing(InstallStep::SettingUpSparse);
                                Ok(())
                            }
                            Err(e) => Err(e),
                        },
                        InstallStep::SettingUpSparse => {
                            match self.setup_sparse_checkout(terminal) {
                                Ok(_) => {
                                    self.state = AppState::Installing(InstallStep::MovingFiles);
                                    Ok(())
                                }
                                Err(e) => Err(e),
                            }
                        }
                        InstallStep::MovingFiles => match self.move_files() {
                            Ok(_) => {
                                self.state =
                                    AppState::Installing(InstallStep::UpdatingDependencies);
                                Ok(())
                            }
                            Err(e) => Err(e),
                        },
                        InstallStep::UpdatingDependencies => match self.update_dependencies() {
                            Ok(_) => {
                                self.state = AppState::Installing(InstallStep::SettingUpForge);
                                Ok(())
                            }
                            Err(e) => Err(e),
                        },
                        InstallStep::SettingUpForge => match self.setup_forge(terminal) {
                            Ok(_) => {
                                self.state = AppState::Success;
                                self.status_message = format!(
                                    "✓ Project '{}' created successfully!",
                                    self.project_name
                                );
                                Ok(())
                            }
                            Err(e) => Err(e),
                        },
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
                AppState::Testing(_) => {
                    if let Err(e) = self.handle_test_step(terminal) {
                        self.add_output(format!("Error: {}", e));
                        self.cleanup_test();
                        self.state = AppState::TestMenu;
                    }
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
        let chunks = match self.state {
            AppState::Success => Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([
                    Constraint::Length(1),   // Status message
                    Constraint::Length(1),   // Input field
                    Constraint::Ratio(1, 2), // Success message gets half the remaining space
                    Constraint::Ratio(1, 2), // Command output gets the other half
                ])
                .split(inner_area),
            AppState::TestMenu | AppState::ConfirmOverwrite => {
                Layout::default() // Add ConfirmOverwrite here
                    .direction(Direction::Vertical)
                    .margin(1)
                    .constraints([
                        Constraint::Length(1),   // Status message
                        Constraint::Length(1),   // Input field
                        Constraint::Ratio(1, 2), // Menu gets half the remaining space
                        Constraint::Ratio(1, 2), // Command output gets the other half
                    ])
                    .split(inner_area)
            }
            _ => Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([
                    Constraint::Length(1), // Status message
                    Constraint::Length(1), // Input field
                    Constraint::Length(3), // Progress/menu area
                    Constraint::Min(0),    // Command output
                ])
                .split(inner_area),
        };

        // Render status message
        let status = Paragraph::new(self.status_message.clone());
        frame.render_widget(status, chunks[0]);

        // Render input field when in EnteringProjectName state
        if let AppState::EnteringProjectName = self.state {
            let cursor_blink = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
                / 500)
                % 2
                == 0;

            let input_text = format!(
                "{}{}",
                self.project_name,
                if cursor_blink { "█" } else { " " }
            );

            let input_lines = vec![
                Line::from(input_text).style(Style::default().fg(Color::Yellow)),
                Line::from(""), // Add a blank line for spacing
                Line::from("Press Esc to exit").style(Style::default().fg(Color::Gray)),
            ];

            let input = Paragraph::new(input_lines).block(Block::default().borders(Borders::NONE));
            frame.render_widget(input, chunks[1]);
        }

        // Show dependency status
        if let AppState::CheckingDependencies = self.state {
            let deps_status = vec![
                format!("Rust: {}", if self.rust_installed { "✓" } else { "..." }),
                format!(
                    "Foundry: {}",
                    if self.foundry_installed { "✓" } else { "..." }
                ),
                format!(
                    "RISC0: {}",
                    self.risc0_version.as_ref().map_or("...", |v| v)
                ),
            ]
            .join("\n");

            let deps = Paragraph::new(deps_status).style(Style::default().fg(Color::Gray));
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

            let progress_widget =
                Paragraph::new(progress_text).block(Block::default().borders(Borders::NONE));
            frame.render_widget(progress_widget, chunks[2]);
        }

        // Add confirmation dialog display
        if let AppState::ConfirmOverwrite = self.state {
            let confirm_text = vec![
                Line::from("Directory already exists!")
                    .style(Style::default().fg(Color::Yellow).bold()),
                Line::from(""),
                Line::from("Use ↑↓ arrows to select, Enter to confirm:")
                    .style(Style::default().fg(Color::Gray)),
                Line::from(""),
                Line::from(if self.confirm_menu_item == 0 {
                    "▶ Continue (overwrite)"
                } else {
                    "  Continue (overwrite)"
                })
                .style(if self.confirm_menu_item == 0 {
                    Style::default().fg(Color::Yellow).bold()
                } else {
                    Style::default()
                }),
                Line::from(""), // Add spacing between options
                Line::from(if self.confirm_menu_item == 1 {
                    "▶ Go to testing toolbox"
                } else {
                    "  Go to testing toolbox"
                })
                .style(if self.confirm_menu_item == 1 {
                    Style::default().fg(Color::Yellow).bold()
                } else {
                    Style::default()
                }),
                Line::from(""), // Add spacing between options
                Line::from(if self.confirm_menu_item == 2 {
                    "▶ Exit"
                } else {
                    "  Exit"
                })
                .style(if self.confirm_menu_item == 2 {
                    Style::default().fg(Color::Yellow).bold()
                } else {
                    Style::default()
                }),
            ];

            let confirm =
                Paragraph::new(confirm_text).block(Block::default().borders(Borders::NONE));
            frame.render_widget(confirm, chunks[2]);
        }

        // Add success message display
        if let AppState::Success = self.state {
            let success_text = vec![
                Line::from(""),
                Line::from("✨ Success! ✨").style(Style::default().fg(Color::Green).bold()),
                Line::from(""),
                Line::from(format!(
                    "Project '{}' has been created successfully!",
                    self.project_name
                )),
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
            let output_text = self
                .command_output
                .iter()
                .map(|line| Line::from(line.as_str()))
                .collect::<Vec<_>>();

            let output = Paragraph::new(output_text)
                .block(
                    Block::default()
                        .title("Command Output")
                        .borders(Borders::ALL),
                )
                .scroll((self.output_scroll, 0))
                .wrap(Wrap { trim: true });

            frame.render_widget(output, chunks[3]);
        }

        if let AppState::TestMenu = self.state {
            let menu_text = vec![
                Line::from("End-to-End Test Menu").style(Style::default().bold()),
                Line::from(""),
                Line::from("Use ↑↓ arrows to select, Enter to confirm:")
                    .style(Style::default().fg(Color::Gray)),
                Line::from(""),
                Line::from(if self.selected_menu_item == 0 {
                    "▶ 🔧 Run end-to-end test with Anvil"
                } else {
                    "  🔧 Run end-to-end test with Anvil"
                })
                .style(if self.selected_menu_item == 0 {
                    Style::default().fg(Color::Yellow).bold()
                } else {
                    Style::default()
                }),
                Line::from(""),
                Line::from(if self.selected_menu_item == 1 {
                    "▶ 🚪 Exit"
                } else {
                    "  🚪 Exit"
                })
                .style(if self.selected_menu_item == 1 {
                    Style::default().fg(Color::Yellow).bold()
                } else {
                    Style::default()
                }),
            ];

            let menu = Paragraph::new(menu_text).block(Block::default().borders(Borders::NONE));
            frame.render_widget(menu, chunks[2]);
        }
    }
}
