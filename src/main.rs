use anyhow::{Context as AnyhowContext, Result};
use eframe::{egui, Frame};
use egui::{Color32, Context as EguiContext, RichText, Ui};
use rfd::FileDialog;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use futures::future::join_all;

const HKXCMD_EXE: &[u8] = include_bytes!("hkxcmd.exe");
const HKXC_EXE: &[u8] = include_bytes!("hkxc.exe");
const HKXCONV_EXE: &[u8] = include_bytes!("hkxconv.exe");
const SSE_TO_LE_HKO: &[u8] = include_bytes!("_SSEtoLE.hko");
const HAVOK_BEHAVIOR_POST_PROCESS_EXE: &[u8] = include_bytes!("HavokBehaviorPostProcess.exe");

#[derive(PartialEq, Clone, Copy, Debug)]
enum ConverterTool {
    HkxCmd,
    HkxC,
    HkxConv,
    Hct,
    HavokBehaviorPostProcess,
}

impl ConverterTool {
    fn label(&self) -> &'static str {
        match self {
            ConverterTool::HkxCmd => "hkxcmd",
            ConverterTool::HkxC => "hkxc",
            ConverterTool::HkxConv => "hkxconv",
            ConverterTool::Hct => "HCT",
            ConverterTool::HavokBehaviorPostProcess => "HavokBehaviorPostProcess",
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
                ConverterTool::Hct => "All (HKX only)",
                ConverterTool::HavokBehaviorPostProcess => "All (HKX only)",
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
    sse_to_le_hko_path: PathBuf,
    havok_behavior_post_process_path: PathBuf,
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
            sse_to_le_hko_path: PathBuf::new(),
            havok_behavior_post_process_path: PathBuf::new(),
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
    sse_to_le_hko_path: PathBuf,
    havok_behavior_post_process_path: PathBuf,
}

impl TempConversionContext {
    async fn run_conversion_tool(&self, input: &Path, output: &Path) -> Result<()> {
        let mut command = match self.converter_tool {
            ConverterTool::HkxCmd => Command::new(&self.hkxcmd_path),
            ConverterTool::HkxC => Command::new(&self.hkxc_path),
            ConverterTool::HkxConv => Command::new(&self.hkxconv_path),
            ConverterTool::Hct => Command::new("hctStandAloneFilterManager.exe"),
            ConverterTool::HavokBehaviorPostProcess => Command::new(&self.havok_behavior_post_process_path),
        };
        
        let tool_name = match self.converter_tool {
            ConverterTool::HkxCmd => "hkxcmd",
            ConverterTool::HkxC => "hkxc",
            ConverterTool::HkxConv => "hkxconv",
            ConverterTool::Hct => "hctStandAloneFilterManager",
            ConverterTool::HavokBehaviorPostProcess => "HavokBehaviorPostProcess",
        };

        // Convert paths to absolute paths to avoid issues with paths starting with '-'
        // Use absolute paths but avoid canonicalize() which can add \\?\ prefix on Windows
        let input_absolute = if input.is_absolute() { 
            input.to_path_buf() 
        } else { 
            std::env::current_dir().unwrap_or_default().join(input) 
        };
        let output_absolute = if output.is_absolute() { 
            output.to_path_buf() 
        } else { 
            std::env::current_dir().unwrap_or_default().join(output) 
        };
        
        // Also handle skeleton file if it exists
        let skeleton_absolute = self.skeleton_file.as_ref().map(|skeleton| {
            if skeleton.is_absolute() { 
                skeleton.to_path_buf() 
            } else { 
                std::env::current_dir().unwrap_or_default().join(skeleton) 
            }
        });
        
        // Set the command based on conversion mode
        match self.conversion_mode {
            ConversionMode::Regular => {
                if self.converter_tool != ConverterTool::Hct && self.converter_tool != ConverterTool::HavokBehaviorPostProcess {
                    command.arg("convert");
                }
                // HCT and HavokBehaviorPostProcess don't need a command argument
            }
            ConversionMode::KfToHkx => {
                if self.converter_tool != ConverterTool::Hct {
                    command.arg("ConvertKF");
                }
                // HCT doesn't support KF conversion
            }
            ConversionMode::HkxToKf => {
                if self.converter_tool != ConverterTool::Hct {
                    command.arg("exportkf");
                }
                // HCT doesn't support KF conversion
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
            (ConversionMode::Regular, ConverterTool::Hct) => {
                // For HCT, create a unique temporary directory for this conversion
                let temp_dir = tempfile::Builder::new()
                    .prefix("hct_conversion_")
                    .tempdir()
                    .context("Failed to create temporary directory for HCT conversion")?;
                
                // HCT only supports SSE to LE conversion
                let source_hko_path = &self.sse_to_le_hko_path;
                
                // Copy the .hko file to the temporary directory
                let hko_filename = source_hko_path.file_name().unwrap();
                let temp_hko_path = temp_dir.path().join(hko_filename);
                fs::copy(source_hko_path, &temp_hko_path)
                    .context("Failed to copy .hko file to temporary directory")?;
                
                println!("HCT temp dir: {:?}, using .hko: {:?}", temp_dir.path(), hko_filename);
                
                // Set working directory to temp directory and use relative .hko filename
                command.current_dir(temp_dir.path());
                command.arg(&input_absolute);
                command.arg("-s");
                command.arg(hko_filename);  // Just the filename, not full path
                
                // Execute the command
                let cmd_output = command.output().await.context("Failed to execute HCT converter tool")?;
                let stderr = String::from_utf8_lossy(&cmd_output.stderr);

                if !cmd_output.status.success() {
                    return Err(anyhow::anyhow!("{} failed: {}", tool_name, stderr));
                }
                
                // HCT creates "filename.hkx" in the same directory as the .hko file
                let hct_output_file = temp_dir.path().join("filename.hkx");
                
                // Debug: List all files in temp directory
                println!("Temp directory contents:");
                if let Ok(entries) = fs::read_dir(temp_dir.path()) {
                    for entry in entries.flatten() {
                        println!("  {:?}", entry.path());
                    }
                } else {
                    println!("  Failed to read temp directory");
                }
                
                if !hct_output_file.exists() {
                    return Err(anyhow::anyhow!("HCT did not produce expected output file: {:?}", hct_output_file));
                }
                
                println!("HCT output file exists: {:?}", hct_output_file);
                println!("Target output path: {:?}", output_absolute);
                
                // Create output directory if it doesn't exist
                if let Some(parent) = output_absolute.parent() {
                    println!("Creating output directory: {:?}", parent);
                    fs::create_dir_all(parent).context("Failed to create output directory")?;
                }
                
                // Check if target file already exists and remove it if necessary
                if output_absolute.exists() {
                    println!("Target file already exists, removing: {:?}", output_absolute);
                    fs::remove_file(&output_absolute).context("Failed to remove existing target file")?;
                }
                
                // Move the HCT output file directly to the final location
                // The output_absolute path already includes any suffix/extension modifications
                match fs::rename(&hct_output_file, &output_absolute) {
                    Ok(_) => {
                        println!("Successfully moved HCT output to: {:?}", output_absolute);
                    }
                    Err(e) => {
                        // If rename fails, try copy + delete as fallback
                        println!("Rename failed ({}), trying copy + delete fallback", e);
                        fs::copy(&hct_output_file, &output_absolute)
                            .context("Failed to copy HCT output file to final location")?;
                        fs::remove_file(&hct_output_file)
                            .context("Failed to remove temporary HCT output file after copy")?;
                        println!("Successfully copied HCT output to: {:?}", output_absolute);
                    }
                }
                
                println!("HCT conversion complete: {:?} -> {:?}", input_absolute, output_absolute);
                
                // temp_dir will be automatically cleaned up when it goes out of scope
                return Ok(());
            }
            (ConversionMode::KfToHkx, ConverterTool::Hct) => {
                return Err(anyhow::anyhow!("HCT does not support KF conversion"));
            }
            (ConversionMode::HkxToKf, ConverterTool::Hct) => {
                return Err(anyhow::anyhow!("HCT does not support KF conversion"));
            }
            (ConversionMode::Regular, ConverterTool::HavokBehaviorPostProcess) => {
                // HavokBehaviorPostProcess only supports HKX input files and SSE output
                if input_absolute.extension().map_or(true, |ext| ext != "hkx") {
                    return Err(anyhow::anyhow!("HavokBehaviorPostProcess requires an HKX input file."));
                }
                
                // HavokBehaviorPostProcess modifies files in-place, so we need to copy the input to output first
                println!("Input path: {:?}", input_absolute);
                println!("Output path: {:?}", output_absolute);
                println!("Input exists: {}", input_absolute.exists());
                println!("Output parent exists: {}", output_absolute.parent().map_or(false, |p| p.exists()));
                println!("Copying input file to output location: {:?} -> {:?}", input_absolute, output_absolute);
                
                // Check if input and output are the same
                if input_absolute == output_absolute {
                    return Err(anyhow::anyhow!("Input and output paths are the same: {:?}", input_absolute));
                }
                
                // Create output directory if it doesn't exist
                if let Some(parent) = output_absolute.parent() {
                    println!("Creating output directory: {:?}", parent);
                    fs::create_dir_all(parent).context("Failed to create output directory")?;
                }
                
                // Copy input file to output location
                match fs::copy(&input_absolute, &output_absolute) {
                    Ok(bytes_copied) => {
                        println!("Successfully copied {} bytes", bytes_copied);
                    }
                    Err(e) => {
                        println!("Copy failed with error: {:?}", e);
                        return Err(anyhow::anyhow!("Failed to copy input file to output location: {}", e));
                    }
                }
                
                // Check file size before processing
                let file_size_before = fs::metadata(&output_absolute)
                    .context("Failed to get file metadata before processing")?
                    .len();
                println!("File size before HavokBehaviorPostProcess: {} bytes", file_size_before);
                
                // Run HavokBehaviorPostProcess on the output file (modifies in-place)
                command.arg("--platformAmd64");
                // Both input and output are the same file (in-place modification)
                // Don't manually add quotes - let Command handle it
                command.arg(&output_absolute);
                command.arg(&output_absolute);
            }
            (ConversionMode::KfToHkx, ConverterTool::HavokBehaviorPostProcess) => {
                return Err(anyhow::anyhow!("HavokBehaviorPostProcess does not support KF conversion"));
            }
            (ConversionMode::HkxToKf, ConverterTool::HavokBehaviorPostProcess) => {
                return Err(anyhow::anyhow!("HavokBehaviorPostProcess does not support KF conversion"));
            }
        }

        // Print the command being executed for debugging
        println!("EXECUTING COMMAND: {:?} with input: {:?}, output: {:?}", tool_name, input_absolute, output_absolute);
        
        // For HavokBehaviorPostProcess, print the exact command with arguments
        if self.converter_tool == ConverterTool::HavokBehaviorPostProcess {
            println!("HavokBehaviorPostProcess command: {:?}", command);
        }

        let output = command.output().await.context("Failed to execute converter tool")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        
        // For HavokBehaviorPostProcess, print all output for debugging
        if self.converter_tool == ConverterTool::HavokBehaviorPostProcess {
            println!("HavokBehaviorPostProcess exit code: {:?}", output.status.code());
            println!("HavokBehaviorPostProcess stdout: {}", stdout);
            println!("HavokBehaviorPostProcess stderr: {}", stderr);
        }

        if !output.status.success() {
            return Err(anyhow::anyhow!("{} failed with exit code {:?}: stdout: {} stderr: {}", 
                tool_name, output.status.code(), stdout, stderr));
        }
        
        // For HavokBehaviorPostProcess, check if the file size changed
        if self.converter_tool == ConverterTool::HavokBehaviorPostProcess {
            let file_size_after = fs::metadata(&output_absolute)
                .context("Failed to get file metadata after processing")?
                .len();
            println!("File size after HavokBehaviorPostProcess: {} bytes", file_size_after);
            
            if file_size_after == fs::metadata(&input_absolute)
                .context("Failed to get input file metadata")?
                .len() {
                println!("WARNING: Output file size is the same as input file size - conversion may not have worked");
            } else {
                println!("SUCCESS: File size changed, conversion appears to have worked");
            }
        }

        Ok(())
    }
}

impl HkxToolsApp {
    fn new(hkxcmd_path: PathBuf, hkxc_path: PathBuf, hkxconv_path: PathBuf, sse_to_le_hko_path: PathBuf, havok_behavior_post_process_path: PathBuf, tokio_handle: tokio::runtime::Handle) -> Self {
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
            sse_to_le_hko_path,
            havok_behavior_post_process_path,
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
                        match self.converter_tool {
                            ConverterTool::HkxCmd => {
                                path.extension().map_or(false, |ext| ext == "hkx" || ext == "xml" || ext == "kf")
                            }
                            ConverterTool::HkxC | ConverterTool::HkxConv => {
                                // hkxc and hkxconv don't support KF files
                                path.extension().map_or(false, |ext| ext == "hkx" || ext == "xml")
                            }
                            ConverterTool::Hct => {
                                // HCT doesn't support KF or XML files
                                path.extension().map_or(false, |ext| ext == "hkx")
                            }
                            ConverterTool::HavokBehaviorPostProcess => {
                                // HavokBehaviorPostProcess only supports HKX files
                                path.extension().map_or(false, |ext| ext == "hkx")
                            }
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
                        match self.converter_tool {
                            ConverterTool::HkxCmd => {
                                path.extension().map_or(false, |ext| ext == "hkx" || ext == "xml" || ext == "kf")
                            }
                            ConverterTool::HkxC | ConverterTool::HkxConv => {
                                // hkxc and hkxconv don't support KF files
                                path.extension().map_or(false, |ext| ext == "hkx" || ext == "xml")
                            }
                            ConverterTool::Hct => {
                                // HCT doesn't support KF or XML files
                                path.extension().map_or(false, |ext| ext == "hkx")
                            }
                            ConverterTool::HavokBehaviorPostProcess => {
                                // HavokBehaviorPostProcess only supports HKX files
                                path.extension().map_or(false, |ext| ext == "hkx")
                            }
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
                match self.converter_tool {
                    ConverterTool::HkxCmd => {
                        file_path.extension().map_or(false, |ext| ext == "hkx" || ext == "xml" || ext == "kf")
                    }
                    ConverterTool::HkxC | ConverterTool::HkxConv => {
                        // hkxc and hkxconv don't support KF files
                        file_path.extension().map_or(false, |ext| ext == "hkx" || ext == "xml")
                    }
                    ConverterTool::Hct => {
                        // HCT doesn't support KF or XML files
                        file_path.extension().map_or(false, |ext| ext == "hkx")
                    }
                    ConverterTool::HavokBehaviorPostProcess => {
                        // HavokBehaviorPostProcess only supports HKX files
                        file_path.extension().map_or(false, |ext| ext == "hkx")
                    }
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
                            ConverterTool::Hct | ConverterTool::HavokBehaviorPostProcess => "Supports: HKX files",
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
        let sse_to_le_hko_path = self.sse_to_le_hko_path.clone();
        let havok_behavior_post_process_path = self.havok_behavior_post_process_path.clone();

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
                sse_to_le_hko_path,
                havok_behavior_post_process_path,
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
        sse_to_le_hko_path: PathBuf,
        havok_behavior_post_process_path: PathBuf,
        progress_tx: mpsc::UnboundedSender<ConversionProgress>,
        mut cancel_rx: oneshot::Receiver<()>,
    ) -> Result<()> {
        let total_files = input_paths.len();
        
        // HCT can now process asynchronously with isolated temp directories
        println!("Processing {} files with {}", total_files, match converter_tool {
            ConverterTool::Hct => "HCT (using isolated temp directories)",
            ConverterTool::HavokBehaviorPostProcess => "HavokBehaviorPostProcess",
            _ => "concurrent processing"
        });
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
                sse_to_le_hko_path: sse_to_le_hko_path.clone(),
                havok_behavior_post_process_path: havok_behavior_post_process_path.clone(),
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
                    for tool in [ConverterTool::HkxCmd, ConverterTool::HkxC, ConverterTool::HkxConv, ConverterTool::Hct, ConverterTool::HavokBehaviorPostProcess] {
                        if ui
                            .selectable_label(self.converter_tool == tool, tool.label())
                            .clicked()
                        {
                            self.converter_tool = tool;
                            // Reset to regular mode if hkxc, hkxconv, HCT, or HavokBehaviorPostProcess is selected and we're in KF mode
                            if (tool == ConverterTool::HkxC || tool == ConverterTool::HkxConv || tool == ConverterTool::Hct || tool == ConverterTool::HavokBehaviorPostProcess) && self.conversion_mode != ConversionMode::Regular {
                                self.conversion_mode = ConversionMode::Regular;
                            }
                            // Reset input file extension if hkxc, hkxconv, HCT, or HavokBehaviorPostProcess is selected and current filter is KF
                            if (tool == ConverterTool::HkxC || tool == ConverterTool::HkxConv || tool == ConverterTool::Hct || tool == ConverterTool::HavokBehaviorPostProcess) && self.input_file_extension == InputFileExtension::Kf {
                                self.input_file_extension = InputFileExtension::Hkx;
                            }
                            // Reset input file extension if HCT or HavokBehaviorPostProcess is selected and current filter is XML
                            if (tool == ConverterTool::Hct || tool == ConverterTool::HavokBehaviorPostProcess) && self.input_file_extension == InputFileExtension::Xml {
                                self.input_file_extension = InputFileExtension::Hkx;
                            }
                            // Reset output format if hkxconv is selected and current format is Skyrim LE
                            if tool == ConverterTool::HkxConv && self.output_format == OutputFormat::SkyrimLE {
                                self.output_format = OutputFormat::SkyrimSE;
                            }
                            // Reset output format if HCT is selected and current format is not LE
                            if tool == ConverterTool::Hct && (self.output_format == OutputFormat::SkyrimSE || self.output_format == OutputFormat::Xml) {
                                self.output_format = OutputFormat::SkyrimLE;
                            }
                            // Reset output format if HavokBehaviorPostProcess is selected and current format is not SSE
                            if tool == ConverterTool::HavokBehaviorPostProcess && (self.output_format == OutputFormat::SkyrimLE || self.output_format == OutputFormat::Xml) {
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
                                (ConversionMode::KfToHkx, ConverterTool::Hct) => false,
                                (ConversionMode::HkxToKf, ConverterTool::Hct) => false,
                                (ConversionMode::KfToHkx, ConverterTool::HavokBehaviorPostProcess) => false,
                                (ConversionMode::HkxToKf, ConverterTool::HavokBehaviorPostProcess) => false,
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
                    let available_filters = match self.converter_tool {
                        ConverterTool::HkxCmd => {
                            vec![
                                InputFileExtension::All,
                                InputFileExtension::Hkx,
                                InputFileExtension::Xml,
                                InputFileExtension::Kf,
                            ]
                        }
                        ConverterTool::HkxC | ConverterTool::HkxConv => {
                            // hkxc and hkxconv don't support KF files
                            vec![
                                InputFileExtension::All,
                                InputFileExtension::Hkx,
                                InputFileExtension::Xml,
                            ]
                        }
                        ConverterTool::Hct => {
                            // HCT doesn't support KF or XML files
                            vec![
                                InputFileExtension::All,
                                InputFileExtension::Hkx,
                            ]
                        }
                        ConverterTool::HavokBehaviorPostProcess => {
                            // HavokBehaviorPostProcess only supports HKX files
                            vec![
                                InputFileExtension::All,
                                InputFileExtension::Hkx,
                            ]
                        }
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
        
        // Show HCT processing note
        // if self.converter_tool == ConverterTool::Hct {
        //     ui.horizontal(|ui| {
        //         ui.label(RichText::new("ℹ️ HCT files use isolated temp directories for safe concurrent processing").color(Color32::from_rgb(100, 100, 100)).size(12.0));
        //     });
        // }
        
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
                ConverterTool::Hct => {
                    // HCT only supports LE conversion
                    vec![
                        OutputFormat::SkyrimLE,
                    ]
                }
                ConverterTool::HavokBehaviorPostProcess => {
                    // HavokBehaviorPostProcess only supports SSE
                    vec![
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
            if self.converter_tool == ConverterTool::Hct && (self.output_format == OutputFormat::SkyrimSE || self.output_format == OutputFormat::Xml) {
                self.output_format = OutputFormat::SkyrimLE;
            }
            if self.converter_tool == ConverterTool::HavokBehaviorPostProcess && (self.output_format == OutputFormat::SkyrimLE || self.output_format == OutputFormat::Xml) {
                self.output_format = OutputFormat::SkyrimSE;
            }
            
            // Reset to a valid filter if current selection is not available
            if (self.converter_tool == ConverterTool::HkxC || self.converter_tool == ConverterTool::HkxConv || self.converter_tool == ConverterTool::Hct || self.converter_tool == ConverterTool::HavokBehaviorPostProcess) && self.input_file_extension == InputFileExtension::Kf {
                self.input_file_extension = InputFileExtension::Hkx;
            }
            if (self.converter_tool == ConverterTool::Hct || self.converter_tool == ConverterTool::HavokBehaviorPostProcess) && self.input_file_extension == InputFileExtension::Xml {
                self.input_file_extension = InputFileExtension::Hkx;
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

    // Write hkxcmd.exe, hkxc.exe, hkxconv.exe, and HCT .hko file to a temporary location
    let temp_dir = tempfile::Builder::new()
        .prefix("hkxtools_")
        .tempdir()
        .unwrap();
    
    let hkxcmd_path = temp_dir.path().join("hkxcmd.exe");
    let hkxc_path = temp_dir.path().join("hkxc.exe");
    let hkxconv_path = temp_dir.path().join("hkxconv.exe");
    let sse_to_le_hko_path = temp_dir.path().join("_SSEtoLE.hko");
    let havok_behavior_post_process_path = temp_dir.path().join("HavokBehaviorPostProcess.exe");
    
    fs::write(&hkxcmd_path, HKXCMD_EXE).unwrap();
    fs::write(&hkxc_path, HKXC_EXE).unwrap();
    fs::write(&hkxconv_path, HKXCONV_EXE).unwrap();
    fs::write(&sse_to_le_hko_path, SSE_TO_LE_HKO).unwrap();
    fs::write(&havok_behavior_post_process_path, HAVOK_BEHAVIOR_POST_PROCESS_EXE).unwrap();

    println!("Extracted hkxcmd.exe to: {:?}", hkxcmd_path);
    println!("Extracted hkxc.exe to: {:?}", hkxc_path);
    println!("Extracted hkxconv.exe to: {:?}", hkxconv_path);
    println!("Extracted _SSEtoLE.hko to: {:?}", sse_to_le_hko_path);
    println!("Extracted HavokBehaviorPostProcess.exe to: {:?}", havok_behavior_post_process_path);
    println!("HCT will be called from PATH as: hctStandAloneFilterManager.exe");

    // Window width and height
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([600.0, 680.0]),
        ..Default::default()
    };
    
    // Keep temp_dir alive for the entire application lifetime
    let _temp_dir_guard = temp_dir;
    
    eframe::run_native(
        "Composite HKX Conversion GUI",
        options,
        Box::new(move |_cc| Ok(Box::new(HkxToolsApp::new(hkxcmd_path, hkxc_path, hkxconv_path, sse_to_le_hko_path, havok_behavior_post_process_path, tokio_handle)))),
    )
}
