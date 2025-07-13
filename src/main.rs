use anyhow::{Context as AnyhowContext, Result};
use eframe::{egui, Frame};
use egui::{Color32, Context as EguiContext, RichText, Ui};
use rfd::FileDialog;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use futures::future::join_all;

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

#[derive(Debug, Clone)]
enum ConversionStatus {
    Idle,
    Running { current_file: String, progress: usize, total: usize },
    Completed { message: String },
    Error { message: String },
}

#[derive(Debug)]
struct ConversionProgress {
    current_file: String,
    file_index: usize,
    total_files: usize,
    status: ConversionStatus,
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
    // Async operation fields
    conversion_status: ConversionStatus,
    progress_rx: Option<mpsc::UnboundedReceiver<ConversionProgress>>,
    cancel_tx: Option<oneshot::Sender<()>>,
    tokio_handle: tokio::runtime::Handle,
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
            conversion_status: ConversionStatus::Idle,
            progress_rx: None,
            cancel_tx: None,
            tokio_handle: tokio::runtime::Handle::current(),
        }
    }
}

// Temporary context for async conversion operations
struct TempConversionContext {
    converter_tool: ConverterTool,
    conversion_mode: ConversionMode,
    output_format: OutputFormat,
    skeleton_file: Option<PathBuf>,
    hkxcmd_path: PathBuf,
    hkxc_path: PathBuf,
    hkxconv_path: PathBuf,
}

impl TempConversionContext {
    async fn run_conversion_tool(&self, input: &Path, output: &Path) -> Result<()> {
        let (executable_path, tool_name) = match self.converter_tool {
            ConverterTool::HkxCmd => (&self.hkxcmd_path, "hkxcmd"),
            ConverterTool::HkxC => (&self.hkxc_path, "hkxc"),
            ConverterTool::HkxConv => (&self.hkxconv_path, "hkxconv"),
        };

        // Convert paths to absolute paths to avoid issues with paths starting with '-'
        let input_absolute = input.canonicalize().unwrap_or_else(|_| input.to_path_buf());
        let output_absolute = output.canonicalize().unwrap_or_else(|_| output.to_path_buf());
        
        // Also handle skeleton file if it exists
        let skeleton_absolute = self.skeleton_file.as_ref().map(|skeleton| {
            skeleton.canonicalize().unwrap_or_else(|_| skeleton.to_path_buf())
        });

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
                command.arg("-i").arg(&input_absolute);
                command.arg("-o").arg(&output_absolute);
                command.arg(format!("-v:{}", match self.output_format {
                    OutputFormat::Xml => "XML",
                    OutputFormat::SkyrimLE => "WIN32",
                    OutputFormat::SkyrimSE => "AMD64",
                }));
            }
            (ConversionMode::Regular, ConverterTool::HkxC) => {
                command.arg("--input").arg(&input_absolute);
                command.arg("--output").arg(&output_absolute);
                command.arg("--format").arg(match self.output_format {
                    OutputFormat::Xml => "xml",
                    OutputFormat::SkyrimLE => "win32",
                    OutputFormat::SkyrimSE => "amd64",
                });
            }
            (ConversionMode::KfToHkx, ConverterTool::HkxCmd) => {
                if let Some(skeleton) = &skeleton_absolute {
                    command.arg(skeleton);
                }
                command.arg(&input_absolute);
                command.arg(&output_absolute);
                command.arg(format!("-v:{}", match self.output_format {
                    OutputFormat::Xml => "XML",
                    OutputFormat::SkyrimLE => "WIN32",
                    OutputFormat::SkyrimSE => "AMD64",
                }));
            }
            (ConversionMode::HkxToKf, ConverterTool::HkxCmd) => {
                if let Some(skeleton) = &skeleton_absolute {
                    command.arg(skeleton);
                }
                command.arg(&input_absolute);
                command.arg(&output_absolute);
            }
            (ConversionMode::KfToHkx, ConverterTool::HkxC) => {
                return Err(anyhow::anyhow!("hkxc does not support KF conversion"));
            }
            (ConversionMode::HkxToKf, ConverterTool::HkxC) => {
                return Err(anyhow::anyhow!("hkxc does not support KF conversion"));
            }
            (ConversionMode::Regular, ConverterTool::HkxConv) => {
                command.arg("convert");
                command.arg(&input_absolute);
                command.arg(&output_absolute);
                command.arg("-v").arg(match self.output_format {
                    OutputFormat::Xml => "xml",
                    OutputFormat::SkyrimLE => "hkx",
                    OutputFormat::SkyrimSE => "hkx",
                });
            }
            (ConversionMode::KfToHkx, ConverterTool::HkxConv) => {
                return Err(anyhow::anyhow!("hkxconv does not support KF conversion"));
            }
            (ConversionMode::HkxToKf, ConverterTool::HkxConv) => {
                return Err(anyhow::anyhow!("hkxconv does not support KF conversion"));
            }
        }

        // Print the command being executed for debugging
        println!("EXECUTING COMMAND: {:?} with input: {:?}, output: {:?}", executable_path, input_absolute, output_absolute);

        let output = command.output().await.context("Failed to execute converter tool")?;
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            return Err(anyhow::anyhow!("{} failed: {}", tool_name, stderr));
        }

        Ok(())
    }
}

impl HkxToolsApp {
    fn new(hkxcmd_path: PathBuf, hkxc_path: PathBuf, hkxconv_path: PathBuf, tokio_handle: tokio::runtime::Handle) -> Self {
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
            conversion_status: ConversionStatus::Idle,
            progress_rx: None,
            cancel_tx: None,
            tokio_handle,
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

    /// Add a single file to the input files list, checking if it matches the current extension filter
    fn add_file(&mut self, file_path: PathBuf) -> bool {
        if !file_path.is_file() {
            return false;
        }

        let matches = match self.input_file_extension {
            InputFileExtension::All => {
                if self.converter_tool == ConverterTool::HkxCmd {
                    file_path.extension().map_or(false, |ext| ext == "hkx" || ext == "xml" || ext == "kf")
                } else {
                    // hkxc and hkxconv don't support KF files
                    file_path.extension().map_or(false, |ext| ext == "hkx" || ext == "xml")
                }
            }
            InputFileExtension::Hkx => {
                file_path.extension().map_or(false, |ext| ext == "hkx")
            }
            InputFileExtension::Xml => {
                file_path.extension().map_or(false, |ext| ext == "xml")
            }
            InputFileExtension::Kf => {
                file_path.extension().map_or(false, |ext| ext == "kf")
            }
        };

        if matches && !self.input_paths.contains(&file_path) {
            self.input_paths.push(file_path);
            true
        } else {
            false
        }
    }

    /// Process dropped files and add valid ones to the input files list
    fn handle_dropped_files(&mut self, dropped_files: Vec<egui::DroppedFile>) {
        let mut files_added = 0;
        let mut files_skipped = 0;

        for dropped_file in dropped_files {
            if let Some(path) = dropped_file.path {
                if path.is_file() {
                    if self.add_file(path) {
                        files_added += 1;
                    } else {
                        files_skipped += 1;
                    }
                } else if path.is_dir() {
                    // If a directory is dropped, add all files from it (non-recursive)
                    if let Ok(entries) = std::fs::read_dir(&path) {
                        for entry in entries.flatten() {
                            let entry_path = entry.path();
                            if entry_path.is_file() {
                                if self.add_file(entry_path) {
                                    files_added += 1;
                                } else {
                                    files_skipped += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Update output folder if files were added
        if files_added > 0 {
            self.update_output_folder();
        }

        // Print feedback for debugging
        if files_added > 0 || files_skipped > 0 {
            println!("Drag & Drop: Added {} files, skipped {} files", files_added, files_skipped);
        }
    }

    /// Render a visual overlay when files are being dragged over the window
    fn render_drag_drop_overlay(&self, ctx: &EguiContext, hovered_files_count: usize) {
        // Create a semi-transparent overlay covering the entire window
        egui::Area::new("drag_drop_overlay".into())
            .fixed_pos(egui::Pos2::ZERO)
            .show(ctx, |ui| {
                // Get the available screen space
                let screen_rect = ctx.screen_rect();
                
                // Draw semi-transparent background
                ui.allocate_ui_at_rect(screen_rect, |ui| {
                    // Background with semi-transparent blue
                    ui.painter().rect_filled(
                        screen_rect,
                        egui::Rounding::ZERO,
                        Color32::from_rgba_unmultiplied(0, 100, 200, 100), // Semi-transparent blue
                    );
                    
                    // Add animated dashed border for better visual feedback
                    let border_color = Color32::from_rgb(0, 150, 255);
                    let border_width = 4.0;
                    
                    // Create a dashed border effect by drawing multiple smaller rectangles
                    let margin = border_width / 2.0;
                    let inner_rect = screen_rect.shrink(margin);
                    
                    // Draw the main border
                    ui.painter().rect_stroke(
                        inner_rect,
                        egui::Rounding::same(5.0),
                        egui::Stroke::new(border_width, border_color),
                    );
                    
                    // Add an inner glow effect with a slightly smaller rectangle
                    let glow_rect = inner_rect.shrink(border_width);
                    ui.painter().rect_stroke(
                        glow_rect,
                        egui::Rounding::same(5.0),
                        egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 150, 255, 150)),
                    );
                    
                    // Center the content
                    ui.allocate_ui_at_rect(screen_rect, |ui| {
                        ui.centered_and_justified(|ui| {
                            ui.vertical_centered(|ui| {
                                // Create a centered box for the content
                                ui.allocate_ui_with_layout(
                                    egui::Vec2::new(400.0, 300.0),
                                    egui::Layout::top_down(egui::Align::Center),
                                    |ui| {
                                        ui.add_space(20.0);
                                        
                                        // Large drop icon with background
                                        ui.label(RichText::new("⬇").size(80.0).color(Color32::WHITE));
                                        
                                        ui.add_space(15.0);
                                        
                                        // Main drop message
                                        ui.label(
                                            RichText::new("Drop Files Here")
                                                .size(28.0)
                                                .color(Color32::WHITE)
                                                .strong()
                                        );
                                        
                                        ui.add_space(15.0);
                                        
                                        // File count and supported formats
                                        let file_text = if hovered_files_count == 1 {
                                            "1 file ready to drop".to_string()
                                        } else {
                                            format!("{} files ready to drop", hovered_files_count)
                                        };
                                        
                                        ui.label(
                                            RichText::new(file_text)
                                                .size(18.0)
                                                .color(Color32::from_rgb(200, 230, 255))
                                        );
                                        
                                        ui.add_space(10.0);
                                        
                                        // Supported formats
                                        let supported_formats = match self.converter_tool {
                                            ConverterTool::HkxCmd => "Supports: HKX, XML, KF files",
                                            ConverterTool::HkxC | ConverterTool::HkxConv => "Supports: HKX, XML files",
                                        };
                                        
                                        ui.label(
                                            RichText::new(supported_formats)
                                                .size(14.0)
                                                .color(Color32::from_rgb(180, 210, 255))
                                                .italics()
                                        );
                                        
                                        ui.add_space(10.0);
                                        
                                        // Add a subtle hint about folder support
                                        ui.label(
                                            RichText::new("Files and folders are supported")
                                                .size(12.0)
                                                .color(Color32::from_rgb(150, 180, 220))
                                                .italics()
                                        );
                                    }
                                );
                            });
                        });
                    });
                });
            });
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

    fn start_conversion(&mut self) {
        // Validation
        if self.input_paths.is_empty() {
            self.conversion_status = ConversionStatus::Error {
                message: "No input files selected".to_string(),
            };
            return;
        }
        if self.output_folder.is_none() {
            self.conversion_status = ConversionStatus::Error {
                message: "No output folder selected".to_string(),
            };
            return;
        }
        if self.conversion_mode.requires_skeleton() && self.skeleton_file.is_none() {
            self.conversion_status = ConversionStatus::Error {
                message: "Skeleton file is required for animation conversion".to_string(),
            };
            return;
        }

        // Setup channels for progress communication
        let (progress_tx, progress_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = oneshot::channel();
        
        self.progress_rx = Some(progress_rx);
        self.cancel_tx = Some(cancel_tx);
        self.conversion_status = ConversionStatus::Running {
            current_file: "Starting...".to_string(),
            progress: 0,
            total: self.input_paths.len(),
        };

        // Clone data needed for the async task
        let input_paths = self.input_paths.clone();
        let output_folder = self.output_folder.clone().unwrap();
        let skeleton_file = self.skeleton_file.clone();
        let output_suffix = self.output_suffix.clone();
        let output_format = self.output_format;
        let custom_extension = self.custom_extension.clone();
        let conversion_mode = self.conversion_mode;
        let converter_tool = self.converter_tool;
        let hkxcmd_path = self.hkxcmd_path.clone();
        let hkxc_path = self.hkxc_path.clone();
        let hkxconv_path = self.hkxconv_path.clone();

        // Spawn the async conversion task
        self.tokio_handle.spawn(async move {
            let result = Self::run_conversion_async(
                input_paths,
                output_folder,
                skeleton_file,
                output_suffix,
                output_format,
                custom_extension,
                conversion_mode,
                converter_tool,
                hkxcmd_path,
                hkxc_path,
                hkxconv_path,
                progress_tx,
                cancel_rx,
            ).await;

            // The task will complete on its own
            drop(result);
        });
    }

    async fn run_conversion_async(
        input_paths: Vec<PathBuf>,
        output_folder: PathBuf,
        skeleton_file: Option<PathBuf>,
        output_suffix: String,
        output_format: OutputFormat,
        custom_extension: Option<String>,
        conversion_mode: ConversionMode,
        converter_tool: ConverterTool,
        hkxcmd_path: PathBuf,
        hkxc_path: PathBuf,
        hkxconv_path: PathBuf,
        progress_tx: mpsc::UnboundedSender<ConversionProgress>,
        mut cancel_rx: oneshot::Receiver<()>,
    ) -> Result<()> {
        let total_files = input_paths.len();

        // Create all conversion tasks concurrently
        let mut conversion_tasks = Vec::new();
        
        for (index, input_path) in input_paths.iter().enumerate() {
            // Check for cancellation before starting
            if cancel_rx.try_recv().is_ok() {
                let _ = progress_tx.send(ConversionProgress {
                    current_file: "Cancelled".to_string(),
                    file_index: index,
                    total_files,
                    status: ConversionStatus::Error {
                        message: "Conversion cancelled by user".to_string(),
                    },
                });
                return Ok(());
            }

            let output_path = Self::get_output_path_static(
                input_path,
                &output_folder,
                &output_suffix,
                output_format,
                &custom_extension,
                conversion_mode,
            ).context("Failed to determine output path")?;

            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent).context("Failed to create output directories")?;
            }

            println!("Preparing to convert {:?} to {:?}", input_path, output_path);

            // Create a temporary app-like structure for the conversion tool call
            let temp_app = TempConversionContext {
                converter_tool,
                conversion_mode,
                output_format,
                skeleton_file: skeleton_file.clone(),
                hkxcmd_path: hkxcmd_path.clone(),
                hkxc_path: hkxc_path.clone(),
                hkxconv_path: hkxconv_path.clone(),
            };

            // Clone needed data for the async task
            let input_path_clone = input_path.clone();
            let output_path_clone = output_path.clone();
            let progress_tx_clone = progress_tx.clone();
            let file_name = input_path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            // Create individual conversion task
            let conversion_task = tokio::spawn(async move {
                // Send progress update when starting this file
                let _ = progress_tx_clone.send(ConversionProgress {
                    current_file: file_name.clone(),
                    file_index: index,
                    total_files,
                    status: ConversionStatus::Running {
                        current_file: file_name.clone(),
                        progress: index,
                        total: total_files,
                    },
                });

                println!("Starting conversion of {:?}", input_path_clone);

                // Run the actual conversion
                let result = temp_app.run_conversion_tool(&input_path_clone, &output_path_clone).await;

                match result {
                    Ok(_) => {
                        if !output_path_clone.exists() {
                            let error_msg = format!("Output file was not created: {:?}", output_path_clone);
                            let _ = progress_tx_clone.send(ConversionProgress {
                                current_file: file_name.clone(),
                                file_index: index,
                                total_files,
                                status: ConversionStatus::Error {
                                    message: error_msg.clone(),
                                },
                            });
                            return Err(anyhow::anyhow!(error_msg));
                        }

                        println!("Completed conversion of {:?}", input_path_clone);
                        let metadata = fs::metadata(&output_path_clone)?;
                        println!("Output file size: {} bytes", metadata.len());
                        Ok(())
                    }
                    Err(e) => {
                        let _ = progress_tx_clone.send(ConversionProgress {
                            current_file: file_name.clone(),
                            file_index: index,
                            total_files,
                            status: ConversionStatus::Error {
                                message: format!("Failed to convert {}: {}", file_name, e),
                            },
                        });
                        Err(e)
                    }
                }
            });

            conversion_tasks.push(conversion_task);
        }

        // Wait for all conversions to complete concurrently
        let results = join_all(conversion_tasks).await;
        
        // Check results and count successes
        let mut successful_conversions = 0;
        for result in results {
            // Check for cancellation
            if cancel_rx.try_recv().is_ok() {
                let _ = progress_tx.send(ConversionProgress {
                    current_file: "Cancelled".to_string(),
                    file_index: successful_conversions,
                    total_files,
                    status: ConversionStatus::Error {
                        message: "Conversion cancelled by user".to_string(),
                    },
                });
                return Ok(());
            }

            match result {
                Ok(Ok(())) => {
                    successful_conversions += 1;
                }
                Ok(Err(e)) => {
                    return Err(e);
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Task failed: {}", e));
                }
            }
        }

        // Send completion message
        let _ = progress_tx.send(ConversionProgress {
            current_file: "Completed".to_string(),
            file_index: successful_conversions,
            total_files,
            status: ConversionStatus::Completed {
                message: format!("Successfully converted {} of {} files", successful_conversions, total_files),
            },
        });

        Ok(())
    }

    // Static helper method for output path calculation
    fn get_output_path_static(
        input_path: &Path,
        output_folder: &Path,
        output_suffix: &str,
        output_format: OutputFormat,
        custom_extension: &Option<String>,
        conversion_mode: ConversionMode,
    ) -> Option<PathBuf> {
        let file_name = input_path.file_stem()?.to_str()?;
        
        let extension = if let Some(custom_ext) = custom_extension {
            custom_ext.as_str()
        } else {
            match conversion_mode {
                ConversionMode::Regular => output_format.extension(),
                ConversionMode::KfToHkx => "hkx",
                ConversionMode::HkxToKf => "kf",
            }
        };

        let output_name = if output_suffix.is_empty() {
            format!("{}.{}", file_name, extension)
        } else {
            format!("{}_{}.{}", file_name, output_suffix, extension)
        };

        Some(output_folder.join(output_name))
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
        
        // Show drag and drop hint
        ui.horizontal(|ui| {
            ui.label(RichText::new("💡 Tip: You can drag and drop files or folders directly onto this window").color(Color32::from_rgb(100, 100, 100)).size(12.0));
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

        self.handle_conversion(ui);
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
        
        // Check for progress updates
        if let Some(progress_rx) = &mut self.progress_rx {
            while let Ok(progress) = progress_rx.try_recv() {
                self.conversion_status = progress.status;
                // Request repaint to update UI immediately
                ui.ctx().request_repaint();
            }
        }

        // Clone the current status to avoid borrow checker issues
        let current_status = self.conversion_status.clone();
        
        // Display status and controls based on current state
        match current_status {
            ConversionStatus::Idle => {
                if ui.button("Run Conversion").clicked() {
                    self.start_conversion();
                }
            }
            ConversionStatus::Running { current_file, progress, total } => {
                let mut should_cancel = false;
                ui.horizontal(|ui| {
                    ui.label(format!("Converting: {}", current_file));
                    if ui.button("Cancel").clicked() {
                        should_cancel = true;
                    }
                });
                
                if should_cancel {
                    if let Some(cancel_tx) = self.cancel_tx.take() {
                        let _ = cancel_tx.send(());
                    }
                    self.conversion_status = ConversionStatus::Idle;
                }
                
                // Progress bar
                let progress_fraction = if total > 0 { progress as f32 / total as f32 } else { 0.0 };
                let progress_bar = egui::ProgressBar::new(progress_fraction)
                    .text(format!("{}/{}", progress, total));
                ui.add(progress_bar);
                
                // Request continuous repaints while running
                ui.ctx().request_repaint();
            }
            ConversionStatus::Completed { message } => {
                ui.colored_label(Color32::GREEN, format!("OK: {}", message));
                if ui.button("Run Another Conversion").clicked() {
                    self.conversion_status = ConversionStatus::Idle;
                    self.progress_rx = None;
                    self.cancel_tx = None;
                }
            }
            ConversionStatus::Error { message } => {
                ui.colored_label(Color32::RED, format!("NOT OK: {}", message));
                if ui.button("Try Again").clicked() {
                    self.conversion_status = ConversionStatus::Idle;
                    self.progress_rx = None;
                    self.cancel_tx = None;
                }
            }
        }
    }
}

impl eframe::App for HkxToolsApp {
    fn update(&mut self, ctx: &EguiContext, _frame: &mut Frame) {
        // Check if files are being hovered over the window
        let files_being_hovered = ctx.input(|i| i.raw.hovered_files.len() > 0);
        let hovered_files_count = ctx.input(|i| i.raw.hovered_files.len());

        // Handle drag and drop files
        if !ctx.input(|i| i.raw.dropped_files.is_empty()) {
            let dropped_files = ctx.input(|i| i.raw.dropped_files.clone());
            self.handle_dropped_files(dropped_files);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_main_ui(ui);
        });

        // Show drag and drop overlay when files are being hovered
        if files_being_hovered {
            self.render_drag_drop_overlay(ctx, hovered_files_count);
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), eframe::Error> {
    // Create a tokio runtime handle for the GUI
    let tokio_handle = tokio::runtime::Handle::current();

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
        viewport: egui::ViewportBuilder::default().with_inner_size([600.0, 625.0]),
        ..Default::default()
    };
    
    // Keep temp_dir alive for the entire application lifetime
    let _temp_dir_guard = temp_dir;
    
    eframe::run_native(
        "Composite HKX Conversion GUI",
        options,
        Box::new(move |_cc| Ok(Box::new(HkxToolsApp::new(hkxcmd_path, hkxc_path, hkxconv_path, tokio_handle)))),
    )
}
