use anyhow::{Context as AnyhowContext, Result};
use eframe::{egui, Frame};
use egui::{Color32, Context as EguiContext, RichText, Ui};
use rfd::FileDialog;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const HKXCMD_EXE: &[u8] = include_bytes!("hkxcmd.exe");
const HKXC_EXE: &[u8] = include_bytes!("hkxc.exe");
const HKXCONV_EXE: &[u8] = include_bytes!("hkxconv.exe");

#[derive(PartialEq, Clone, Copy, Debug)]
enum ConverterTool {
    HkxCmd,
    HkxC,
    HkxConv,
}

impl ConverterTool {
    fn label(&self) -> &'static str {
        match self {
            ConverterTool::HkxCmd => "hkxcmd",
            ConverterTool::HkxC => "hkxc",
            ConverterTool::HkxConv => "hkxconv",
        }
    }
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum ConversionMode {
    Regular,    // HKX <-> XML
    KfToHkx,    // KF -> HKX (requires skeleton)
    HkxToKf,    // HKX -> KF (requires skeleton)
}

impl ConversionMode {
    fn label(&self) -> &'static str {
        match self {
            ConversionMode::Regular => "Regular (HKX <> XML)",
            ConversionMode::KfToHkx => "KF -> HKX (Animation)",
            ConversionMode::HkxToKf => "HKX -> KF (Animation)",
        }
    }
    
    fn requires_skeleton(&self) -> bool {
        matches!(self, ConversionMode::KfToHkx | ConversionMode::HkxToKf)
    }
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum InputFileExtension {
    All,
    Hkx,
    Xml,
    Kf,
}

impl InputFileExtension {
    fn label_for_tool(&self, tool: ConverterTool) -> &'static str {
        match self {
            InputFileExtension::All => match tool {
                ConverterTool::HkxCmd => "All (HKX, XML, KF)",
                ConverterTool::HkxC => "All (HKX, XML)",
                ConverterTool::HkxConv => "All (HKX, XML)",
            },
            InputFileExtension::Hkx => "HKX only",
            InputFileExtension::Xml => "XML only",
            InputFileExtension::Kf => "KF only",
        }
    }
}

struct HkxToolsApp {
    input_paths: Vec<PathBuf>,
    output_folder: Option<PathBuf>,
    skeleton_file: Option<PathBuf>,
    output_suffix: String,
    output_format: OutputFormat,
    custom_extension: Option<String>,
    input_file_extension: InputFileExtension,
    converter_tool: ConverterTool,
    conversion_mode: ConversionMode,
    hkxcmd_path: PathBuf,
    hkxc_path: PathBuf,
    hkxconv_path: PathBuf,
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
            skeleton_file: None,
            output_suffix: String::new(),
            output_format: OutputFormat::Xml,
            custom_extension: None,
            input_file_extension: InputFileExtension::All,
            converter_tool: ConverterTool::HkxCmd,
            conversion_mode: ConversionMode::Regular,
            hkxcmd_path: PathBuf::new(),
            hkxc_path: PathBuf::new(),
            hkxconv_path: PathBuf::new(),
        }
    }
}

impl HkxToolsApp {
    fn new(hkxcmd_path: PathBuf, hkxc_path: PathBuf, hkxconv_path: PathBuf) -> Self {
        Self {
            input_paths: Vec::new(),
            output_folder: None,
            skeleton_file: None,
            output_suffix: String::new(),
            output_format: OutputFormat::Xml,
            custom_extension: None,
            input_file_extension: InputFileExtension::All,
            converter_tool: ConverterTool::HkxCmd,
            conversion_mode: ConversionMode::Regular,
            hkxcmd_path,
            hkxc_path,
            hkxconv_path,
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
                        if self.converter_tool == ConverterTool::HkxCmd {
                            path.extension().map_or(false, |ext| ext == "hkx" || ext == "xml" || ext == "kf")
                        } else {
                            // hkxc and hkxconv don't support KF files
                            path.extension().map_or(false, |ext| ext == "hkx" || ext == "xml")
                        }
                    }
                    InputFileExtension::Hkx => {
                        path.extension().map_or(false, |ext| ext == "hkx")
                    }
                    InputFileExtension::Xml => {
                        path.extension().map_or(false, |ext| ext == "xml")
                    }
                    InputFileExtension::Kf => {
                        path.extension().map_or(false, |ext| ext == "kf")
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
                        if self.converter_tool == ConverterTool::HkxCmd {
                            path.extension().map_or(false, |ext| ext == "hkx" || ext == "xml" || ext == "kf")
                        } else {
                            // hkxc and hkxconv don't support KF files
                            path.extension().map_or(false, |ext| ext == "hkx" || ext == "xml")
                        }
                    }
                    InputFileExtension::Hkx => {
                        path.extension().map_or(false, |ext| ext == "hkx")
                    }
                    InputFileExtension::Xml => {
                        path.extension().map_or(false, |ext| ext == "xml")
                    }
                    InputFileExtension::Kf => {
                        path.extension().map_or(false, |ext| ext == "kf")
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
        
        // Determine output extension based on conversion mode and custom extension
        let extension = if let Some(custom_ext) = &self.custom_extension {
            custom_ext.as_str()
        } else {
            match self.conversion_mode {
                ConversionMode::Regular => self.output_format.extension(),
                ConversionMode::KfToHkx => "hkx",
                ConversionMode::HkxToKf => "kf",
            }
        };

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
        if self.conversion_mode.requires_skeleton() && self.skeleton_file.is_none() {
            return Err(anyhow::anyhow!("Skeleton file is required for animation conversion"));
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
            ConverterTool::HkxConv => (&self.hkxconv_path, "hkxconv"),
        };

        let mut command = Command::new(executable_path);
        
        // Set the command based on conversion mode
        match self.conversion_mode {
            ConversionMode::Regular => {
                command.arg("convert");
            }
            ConversionMode::KfToHkx => {
                command.arg("convertkf");
            }
            ConversionMode::HkxToKf => {
                command.arg("exportkf");
            }
        }

        // Add arguments based on conversion mode and tool
        match (self.conversion_mode, self.converter_tool) {
            (ConversionMode::Regular, ConverterTool::HkxCmd) => {
                command.arg("-i").arg(input);
                command.arg("-o").arg(output);
                command.arg(format!("-v:{}", match self.output_format {
                    OutputFormat::Xml => "XML",
                    OutputFormat::SkyrimLE => "WIN32",
                    OutputFormat::SkyrimSE => "AMD64",
                }));
            }
            (ConversionMode::Regular, ConverterTool::HkxC) => {
                command.arg("--input").arg(input);
                command.arg("--output").arg(output);
                command.arg("--format").arg(match self.output_format {
                    OutputFormat::Xml => "xml",
                    OutputFormat::SkyrimLE => "win32",
                    OutputFormat::SkyrimSE => "amd64",
                });
            }
            (ConversionMode::KfToHkx, ConverterTool::HkxCmd) => {
                // hkxcmd ConvertKF [skel.hkx] [anim.kf] [anim.hkx]
                if let Some(skeleton) = &self.skeleton_file {
                    command.arg(skeleton);
                }
                command.arg(input);
                command.arg(output);
                command.arg(format!("-v:{}", match self.output_format {
                    OutputFormat::Xml => "XML",
                    OutputFormat::SkyrimLE => "WIN32",
                    OutputFormat::SkyrimSE => "AMD64",
                }));
            }
            (ConversionMode::HkxToKf, ConverterTool::HkxCmd) => {
                // hkxcmd ExportKF [skel.hkx] [anim.hkx] [anim.kf]
                if let Some(skeleton) = &self.skeleton_file {
                    command.arg(skeleton);
                }
                command.arg(input);
                command.arg(output);
                // ExportKF uses different version flags, using defaults for now
            }
            (ConversionMode::KfToHkx, ConverterTool::HkxC) => {
                // hkxc doesn't support KF conversion, this should be disabled in UI
                return Err(anyhow::anyhow!("hkxc does not support KF conversion"));
            }
            (ConversionMode::HkxToKf, ConverterTool::HkxC) => {
                // hkxc doesn't support KF conversion, this should be disabled in UI
                return Err(anyhow::anyhow!("hkxc does not support KF conversion"));
            }
            (ConversionMode::Regular, ConverterTool::HkxConv) => {
                // hkxconv convert <input> <output> -v <hkx|xml>
                command.arg("convert");
                command.arg(input);
                command.arg(output);
                command.arg("-v").arg(match self.output_format {
                    OutputFormat::Xml => "xml",
                    OutputFormat::SkyrimLE => "hkx", // hkxconv only supports SSE/64-bit, but we'll use hkx
                    OutputFormat::SkyrimSE => "hkx", // hkxconv only supports SSE/64-bit
                });
            }
            (ConversionMode::KfToHkx, ConverterTool::HkxConv) => {
                // hkxconv doesn't support KF conversion
                return Err(anyhow::anyhow!("hkxconv does not support KF conversion"));
            }
            (ConversionMode::HkxToKf, ConverterTool::HkxConv) => {
                // hkxconv doesn't support KF conversion
                return Err(anyhow::anyhow!("hkxconv does not support KF conversion"));
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
        println!("Tool: {} | Mode: {:?} | Format: {:?}", tool_name, self.conversion_mode, self.output_format);
        println!("Using embedded executable: {:?}", executable_path);

        let output = command.output().context("Failed to execute converter tool")?;

        // let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        // eprintln!("{} stdout:\n{}", tool_name, stdout);
        // eprintln!("{} stderr:\n{}", tool_name, stderr);

        // println!("{} stdout:\n{}", tool_name, stdout);
        // println!("{} stderr:\n{}", tool_name, stderr);

        if !output.status.success() {
            return Err(anyhow::anyhow!("{} failed: {}", tool_name, stderr));
        }

        Ok(())
    }

    fn render_main_ui(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(10.0);
            ui.heading(
                RichText::new("Composite HKX Conversion Tool")
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
                ui.label("Converter Tool:");
                ui.horizontal(|ui| {
                    for tool in [ConverterTool::HkxCmd, ConverterTool::HkxC, ConverterTool::HkxConv] {
                        if ui
                            .selectable_label(self.converter_tool == tool, tool.label())
                            .clicked()
                        {
                            self.converter_tool = tool;
                            // Reset to regular mode if hkxc or hkxconv is selected and we're in KF mode
                            if (tool == ConverterTool::HkxC || tool == ConverterTool::HkxConv) && self.conversion_mode != ConversionMode::Regular {
                                self.conversion_mode = ConversionMode::Regular;
                            }
                            // Reset input file extension if hkxc or hkxconv is selected and current filter is KF
                            if (tool == ConverterTool::HkxC || tool == ConverterTool::HkxConv) && self.input_file_extension == InputFileExtension::Kf {
                                self.input_file_extension = InputFileExtension::Hkx;
                            }
                            // Reset output format if hkxconv is selected and current format is Skyrim LE
                            if tool == ConverterTool::HkxConv && self.output_format == OutputFormat::SkyrimLE {
                                self.output_format = OutputFormat::SkyrimSE;
                            }
                        }
                    }
                });
                ui.end_row();

                ui.label("Conversion Mode:");
                ui.vertical(|ui| {
                    for mode in [ConversionMode::Regular, ConversionMode::KfToHkx, ConversionMode::HkxToKf] {
                        let is_enabled = match (mode, self.converter_tool) {
                            (ConversionMode::KfToHkx, ConverterTool::HkxC) => false,
                            (ConversionMode::HkxToKf, ConverterTool::HkxC) => false,
                            (ConversionMode::KfToHkx, ConverterTool::HkxConv) => false,
                            (ConversionMode::HkxToKf, ConverterTool::HkxConv) => false,
                            _ => true,
                        };
                        ui.add_enabled_ui(is_enabled, |ui| {
                            if ui.selectable_label(self.conversion_mode == mode, mode.label()).clicked() {
                                self.conversion_mode = mode;
                            }
                        });
                    }
                });
                ui.end_row();

                ui.label("Input File Filter:");
                ui.horizontal(|ui| {
                    let available_filters = if self.converter_tool == ConverterTool::HkxCmd {
                        vec![
                            InputFileExtension::All,
                            InputFileExtension::Hkx,
                            InputFileExtension::Xml,
                            InputFileExtension::Kf,
                        ]
                    } else {
                        // hkxc and hkxconv don't support KF files
                        vec![
                            InputFileExtension::All,
                            InputFileExtension::Hkx,
                            InputFileExtension::Xml,
                        ]
                    };
                    
                    for filter in available_filters {
                        if ui
                            .selectable_label(self.input_file_extension == filter, filter.label_for_tool(self.converter_tool))
                            .clicked()
                        {
                            self.input_file_extension = filter;
                        }
                    }
                    
                    // Reset to a valid filter if current selection is not available
                    if (self.converter_tool == ConverterTool::HkxC || self.converter_tool == ConverterTool::HkxConv) && self.input_file_extension == InputFileExtension::Kf {
                        self.input_file_extension = InputFileExtension::Hkx;
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

                // Skeleton file selection (only show for animation conversion modes)
                if self.conversion_mode.requires_skeleton() {
                    ui.label("Skeleton File:");
                    ui.horizontal(|ui| {
                        if let Some(ref skeleton_file) = self.skeleton_file {
                            ui.label(skeleton_file.file_name().unwrap_or_default().to_string_lossy());
                        } 
                        // else {
                        //     ui.label("(required for animation conversion)");
                        // }
                        if ui.button("Browse").clicked() {
                            if let Some(file) = FileDialog::new()
                                .add_filter("HKX files", &["hkx"])
                                .pick_file()
                            {
                                self.skeleton_file = Some(file);
                            }
                        }
                        if self.skeleton_file.is_some() && ui.button("Clear").clicked() {
                            self.skeleton_file = None;
                        }
                    });
                    ui.end_row();
                }

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
                    // ui.label("(optional - leave empty to use format default)");
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
            let available_formats = match self.converter_tool {
                ConverterTool::HkxCmd | ConverterTool::HkxC => {
                    vec![
                        OutputFormat::Xml,
                        OutputFormat::SkyrimLE,
                        OutputFormat::SkyrimSE,
                    ]
                }
                ConverterTool::HkxConv => {
                    // hkxconv only supports SSE/64-bit HKX and XML
                    vec![
                        OutputFormat::Xml,
                        OutputFormat::SkyrimSE,
                    ]
                }
            };
            
            for format in available_formats {
                if ui
                    .selectable_label(self.output_format == format, format.label())
                    .clicked()
                {
                    self.output_format = format;
                }
            }
            
            // Reset to a valid format if current selection is not available
            if self.converter_tool == ConverterTool::HkxConv && self.output_format == OutputFormat::SkyrimLE {
                self.output_format = OutputFormat::SkyrimSE;
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
    // Write hkxcmd.exe, hkxc.exe, and hkxconv.exe to a temporary location
    let temp_dir = tempfile::Builder::new()
        .prefix("hkxtools_")
        .tempdir()
        .unwrap();
    
    let hkxcmd_path = temp_dir.path().join("hkxcmd.exe");
    let hkxc_path = temp_dir.path().join("hkxc.exe");
    let hkxconv_path = temp_dir.path().join("hkxconv.exe");
    
    fs::write(&hkxcmd_path, HKXCMD_EXE).unwrap();
    fs::write(&hkxc_path, HKXC_EXE).unwrap();
    fs::write(&hkxconv_path, HKXCONV_EXE).unwrap();

    println!("Extracted hkxcmd.exe to: {:?}", hkxcmd_path);
    println!("Extracted hkxc.exe to: {:?}", hkxc_path);
    println!("Extracted hkxconv.exe to: {:?}", hkxconv_path);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([600.0, 605.0]),
        ..Default::default()
    };
    
    // Keep temp_dir alive for the entire application lifetime
    let _temp_dir_guard = temp_dir;
    
    eframe::run_native(
        "Composite HKX Conversion GUI",
        options,
        Box::new(move |_cc| Ok(Box::new(HkxToolsApp::new(hkxcmd_path, hkxc_path, hkxconv_path)))),
    )
}
