use anyhow::{Context as AnyhowContext, Result};
use eframe::{egui, Frame};
use egui::{Color32, Context as EguiContext, RichText, Ui};
use rfd::FileDialog;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const HKXCMD_EXE: &[u8] = include_bytes!("hkxcmd.exe");
const HKXC_EXE: &[u8] = include_bytes!("hkxc.exe");

#[derive(PartialEq, Clone, Copy, Debug)]
enum ConverterTool {
    HkxCmd,
    HkxC,
}

impl ConverterTool {
    fn label(&self) -> &'static str {
        match self {
            ConverterTool::HkxCmd => "hkxcmd",
            ConverterTool::HkxC => "hkxc",
        }
    }
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum InputFileExtension {
    All,
    Hkx,
    Xml,
}

impl InputFileExtension {
    fn label(&self) -> &'static str {
        match self {
            InputFileExtension::All => "All (HKX & XML)",
            InputFileExtension::Hkx => "HKX only",
            InputFileExtension::Xml => "XML only",
        }
    }
}

struct HkxToolsApp {
    input_paths: Vec<PathBuf>,
    output_folder: Option<PathBuf>,
    output_suffix: String,
    output_format: OutputFormat,
    custom_extension: Option<String>,
    input_file_extension: InputFileExtension,
    converter_tool: ConverterTool,
    hkxcmd_path: PathBuf,
    hkxc_path: PathBuf,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum OutputFormat {
    Xml,
    SkyrimLE,
    SkyrimSE,
}

impl OutputFormat {
    fn extension(&self) -> &'static str {
        match self {
            OutputFormat::Xml => "xml",
            OutputFormat::SkyrimLE | OutputFormat::SkyrimSE => "hkx",
        }
    }

    fn label(&self) -> &'static str {
        match self {
            OutputFormat::Xml => "XML",
            OutputFormat::SkyrimLE => "Skyrim LE",
            OutputFormat::SkyrimSE => "Skyrim SE",
        }
    }
}

impl Default for HkxToolsApp {
    fn default() -> Self {
        Self {
            input_paths: Vec::new(),
            output_folder: None,
            output_suffix: String::new(),
            output_format: OutputFormat::Xml,
            custom_extension: None,
            input_file_extension: InputFileExtension::All,
            converter_tool: ConverterTool::HkxCmd,
            hkxcmd_path: PathBuf::new(),
            hkxc_path: PathBuf::new(),
        }
    }
}

impl HkxToolsApp {
    fn new(hkxcmd_path: PathBuf, hkxc_path: PathBuf) -> Self {
        Self {
            input_paths: Vec::new(),
            output_folder: None,
            output_suffix: String::new(),
            output_format: OutputFormat::Xml,
            custom_extension: None,
            input_file_extension: InputFileExtension::All,
            converter_tool: ConverterTool::HkxCmd,
            hkxcmd_path,
            hkxc_path,
        }
    }

    fn add_files_from_folder(&mut self, folder: &Path, recursive: bool) -> Result<()> {
        if recursive {
            self.add_files_recursive(folder)
        } else {
            self.add_files_non_recursive(folder)
        }
    }

    fn add_files_non_recursive(&mut self, folder: &Path) -> Result<()> {
        let entries = fs::read_dir(folder).context("Failed to read directory")?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                let matches = match self.input_file_extension {
                    InputFileExtension::All => {
                        path.extension().map_or(false, |ext| ext == "hkx" || ext == "xml")
                    }
                    InputFileExtension::Hkx => {
                        path.extension().map_or(false, |ext| ext == "hkx")
                    }
                    InputFileExtension::Xml => {
                        path.extension().map_or(false, |ext| ext == "xml")
                    }
                };
                
                if matches && !self.input_paths.contains(&path) {
                    self.input_paths.push(path);
                }
            }
        }
        Ok(())
    }

    fn add_files_recursive(&mut self, folder: &Path) -> Result<()> {
        for entry in walkdir::WalkDir::new(folder).follow_links(true) {
            let entry = entry?;
            let path = entry.path().to_path_buf();
            if path.is_file() {
                let matches = match self.input_file_extension {
                    InputFileExtension::All => {
                        path.extension().map_or(false, |ext| ext == "hkx" || ext == "xml")
                    }
                    InputFileExtension::Hkx => {
                        path.extension().map_or(false, |ext| ext == "hkx")
                    }
                    InputFileExtension::Xml => {
                        path.extension().map_or(false, |ext| ext == "xml")
                    }
                };
                
                if matches && !self.input_paths.contains(&path) {
                    self.input_paths.push(path);
                }
            }
        }
        Ok(())
    }

    fn update_output_folder(&mut self) {
        if let Some(input_path) = self.input_paths.first() {
            self.output_folder = Some(input_path.parent().unwrap_or(Path::new("")).to_path_buf());
        }
    }

    fn get_output_path(&self, input_path: &Path) -> Option<PathBuf> {
        let output_base = self.output_folder.as_ref()?;
        let file_name = input_path.file_stem()?.to_str()?;
        let extension = self.custom_extension.as_ref()
            .map(|s| s.as_str())
            .unwrap_or_else(|| self.output_format.extension());

        let base_dir = if self.input_paths.len() == 1 {
            input_path.parent().unwrap_or(Path::new(""))
        } else {
            self.find_common_parent_dir()
                .unwrap_or_else(|| Path::new(""))
        };

        let relative_path = input_path
            .parent()
            .unwrap_or(Path::new(""))
            .strip_prefix(base_dir)
            .unwrap_or(Path::new(""));

        let output_name = if self.output_suffix.is_empty() {
            format!("{}.{}", file_name, extension)
        } else {
            format!("{}_{}.{}", file_name, self.output_suffix, extension)
        };

        Some(output_base.join(relative_path).join(output_name))
    }

    fn find_common_parent_dir(&self) -> Option<&Path> {
        if self.input_paths.is_empty() {
            return None;
        }

        // get all parent directories
        let parent_dirs: Vec<_> = self
            .input_paths
            .iter()
            .filter_map(|path| path.parent())
            .collect();

        if parent_dirs.is_empty() {
            return None;
        }

        // start with the first parent directory
        let mut common = parent_dirs[0];

        // find the common prefix among all parent directories
        for dir in &parent_dirs[1..] {
            while !dir.starts_with(common) {
                common = common.parent()?;
            }
        }

        Some(common)
    }

    fn run_conversion(&mut self) -> Result<()> {
        if self.input_paths.is_empty() {
            return Err(anyhow::anyhow!("No input files selected"));
        }
        if self.output_folder.is_none() {
            return Err(anyhow::anyhow!("No output folder selected"));
        }

        for input_path in &self.input_paths {
            let output_path = self
                .get_output_path(input_path)
                .context("Failed to determine output path")?;

            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent).context("Failed to create output directories")?;
            }

            println!("Converting {:?} to {:?}", input_path, output_path);

            self.run_conversion_tool(input_path, &output_path)?;

            if !output_path.exists() {
                return Err(anyhow::anyhow!(
                    "Output file was not created: {:?}",
                    output_path
                ));
            }
            println!("Output file created successfully: {:?}", output_path);
            let metadata = fs::metadata(&output_path)?;
            println!("Output file size: {} bytes", metadata.len());
        }

        Ok(())
    }

    fn run_conversion_tool(&self, input: &Path, output: &Path) -> Result<()> {
        let (executable_path, tool_name) = match self.converter_tool {
            ConverterTool::HkxCmd => (&self.hkxcmd_path, "hkxcmd"),
            ConverterTool::HkxC => (&self.hkxc_path, "hkxc"),
        };

        let mut command = Command::new(executable_path);
        command.arg("convert");

        match self.converter_tool {
            ConverterTool::HkxCmd => {
                command.arg("-i").arg(input);
                command.arg("-o").arg(output);
                command.arg(format!("-v:{}", match self.output_format {
                    OutputFormat::Xml => "XML",
                    OutputFormat::SkyrimLE => "WIN32",
                    OutputFormat::SkyrimSE => "AMD64",
                }));
            }
            ConverterTool::HkxC => {
                command.arg("--input").arg(input);
                command.arg("--output").arg(output);
                command.arg("--format").arg(match self.output_format {
                    OutputFormat::Xml => "xml",
                    OutputFormat::SkyrimLE => "win32",
                    OutputFormat::SkyrimSE => "amd64",
                });
            }
        }

        // Print the exact command being executed
        let mut cmd_string = String::new();
        cmd_string.push_str(&executable_path.to_string_lossy());
        for arg in command.get_args() {
            cmd_string.push(' ');
            cmd_string.push_str(&arg.to_string_lossy());
        }
        println!("EXECUTING COMMAND: {}", cmd_string);
        println!("Working directory: {:?}", std::env::current_dir().unwrap_or_default());
        println!("Input file: {:?}", input);
        println!("Output file: {:?}", output);
        println!("Tool: {} | Format: {:?}", tool_name, self.output_format);
        println!("Using embedded executable: {:?}", executable_path);

        let output = command.output().context("Failed to execute converter tool")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        eprintln!("{} stdout:\n{}", tool_name, stdout);
        eprintln!("{} stderr:\n{}", tool_name, stderr);

        println!("{} stdout:\n{}", tool_name, stdout);
        println!("{} stderr:\n{}", tool_name, stderr);

        if !output.status.success() {
            return Err(anyhow::anyhow!("{} failed: {}", tool_name, stderr));
        }

        Ok(())
    }

    fn render_main_ui(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(10.0);
            ui.heading(
                RichText::new("HKX Conversion Tool")
                    .size(24.0)
                    .color(Color32::LIGHT_BLUE),
            );
            ui.add_space(10.0);
        });

        ui.separator();

        egui::Grid::new("main_grid")
            .num_columns(2)
            .spacing([10.0, 10.0])
            .show(ui, |ui| {
                ui.label("Input File Filter:");
                ui.horizontal(|ui| {
                    for filter in [
                        InputFileExtension::All,
                        InputFileExtension::Hkx,
                        InputFileExtension::Xml,
                    ] {
                        if ui
                            .selectable_label(self.input_file_extension == filter, filter.label())
                            .clicked()
                        {
                            self.input_file_extension = filter;
                        }
                    }
                });
                ui.end_row();

                ui.label("Converter Tool:");
                ui.horizontal(|ui| {
                    for tool in [ConverterTool::HkxCmd, ConverterTool::HkxC] {
                        if ui
                            .selectable_label(self.converter_tool == tool, tool.label())
                            .clicked()
                        {
                            self.converter_tool = tool;
                        }
                    }
                });
                ui.end_row();

                ui.label("Input Files:");
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        if ui.button("Browse Files").clicked() {
                            if let Some(paths) = FileDialog::new().pick_files() {
                                self.input_paths = paths;
                                self.update_output_folder();
                            }
                        }
                        if ui.button("Select Folder").clicked() {
                            if let Some(folder) = FileDialog::new().pick_folder() {
                                if let Err(e) = self.add_files_from_folder(&folder, false) {
                                    eprintln!("Error adding files from folder: {}", e);
                                }
                                self.update_output_folder();
                            }
                        }
                        if ui.button("Select Folder (+ Subfolders)").clicked() {
                            if let Some(folder) = FileDialog::new().pick_folder() {
                                if let Err(e) = self.add_files_from_folder(&folder, true) {
                                    eprintln!("Error adding files from folders: {}", e);
                                }
                                self.update_output_folder();
                            }
                        }
                    });
                });
                ui.end_row();

                ui.label("Output Folder:");
                self.render_output_folder(ui);
                ui.end_row();

                ui.label("Output Suffix:");
                ui.text_edit_singleline(&mut self.output_suffix);
                ui.end_row();

                ui.label("Custom Extension:");
                ui.horizontal(|ui| {
                    let mut extension_text = self.custom_extension.as_ref().cloned().unwrap_or_default();
                    if ui.text_edit_singleline(&mut extension_text).changed() {
                        self.custom_extension = if extension_text.is_empty() {
                            None
                        } else {
                            Some(extension_text)
                        };
                    }
                    ui.label("(optional - leave empty to use format default)");
                });
                ui.end_row();

                ui.label("Output Format:");
                self.render_output_format(ui);
                ui.end_row();
            });

        ui.add_space(10.0);

        // Selected Files section outside the grid for more space
        ui.horizontal(|ui| {
            ui.label("Selected Files:");
            ui.label(format!("{} files selected", self.input_paths.len()));
            if ui.button("Clear All").clicked() {
                self.input_paths.clear();
            }
        });
        
        // Scrollable area for file list with maximum height
        let scroll_area_height = 200.0;
        let files_to_remove = ui.allocate_ui_with_layout(
            egui::Vec2::new(ui.available_width(), scroll_area_height),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        let mut files_to_remove = Vec::new();
                        for (index, path) in self.input_paths.iter().enumerate() {
                            ui.horizontal(|ui| {
                                if ui.small_button("❌").clicked() {
                                    files_to_remove.push(index);
                                }
                                ui.label(path.file_name().unwrap_or_default().to_string_lossy());
                            });
                        }
                        files_to_remove
                    })
                    .inner
            },
        ).inner;
        
        // Remove files after the ScrollArea
        for index in files_to_remove.iter().rev() {
            self.input_paths.remove(*index);
        }

        ui.add_space(10.0);

        ui.horizontal(|ui| {
            if ui.button("Run Conversion").clicked() {
                self.handle_conversion(ui);
            }
        });
    }

    fn render_output_folder(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if let Some(ref output_folder) = self.output_folder {
                ui.label(output_folder.to_string_lossy());
            }
            if ui.button("Browse").clicked() {
                if let Some(folder) = FileDialog::new().pick_folder() {
                    self.output_folder = Some(folder);
                }
            }
        });
    }

    fn render_output_format(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for format in [
                OutputFormat::Xml,
                OutputFormat::SkyrimLE,
                OutputFormat::SkyrimSE,
            ] {
                if ui
                    .selectable_label(self.output_format == format, format.label())
                    .clicked()
                {
                    self.output_format = format;
                }
            }
        });
    }

    fn handle_conversion(&mut self, ui: &mut Ui) {
        ui.add_space(5.0);
        match self.run_conversion() {
            Ok(_) => {
                ui.colored_label(Color32::GREEN, "✓ Conversion completed successfully");
            }
            Err(e) => {
                ui.colored_label(Color32::RED, format!("❌ Error during conversion: {}", e));
            }
        }
    }
}

impl eframe::App for HkxToolsApp {
    fn update(&mut self, ctx: &EguiContext, _frame: &mut Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_main_ui(ui);
        });
    }
}

fn main() -> Result<(), eframe::Error> {
    // Write both hkxcmd.exe and hkxc.exe to a temporary location
    let temp_dir = tempfile::Builder::new()
        .prefix("hkxtools_")
        .tempdir()
        .unwrap();
    
    let hkxcmd_path = temp_dir.path().join("hkxcmd.exe");
    let hkxc_path = temp_dir.path().join("hkxc.exe");
    
    fs::write(&hkxcmd_path, HKXCMD_EXE).unwrap();
    fs::write(&hkxc_path, HKXC_EXE).unwrap();

    println!("Extracted hkxcmd.exe to: {:?}", hkxcmd_path);
    println!("Extracted hkxc.exe to: {:?}", hkxc_path);

    let options = eframe::NativeOptions::default();
    
    // Keep temp_dir alive for the entire application lifetime
    let _temp_dir_guard = temp_dir;
    
    eframe::run_native(
        "HKX Tools GUI",
        options,
        Box::new(move |_cc| Ok(Box::new(HkxToolsApp::new(hkxcmd_path, hkxc_path)))),
    )
}
