1. The Filter Manager

Asset processing in the Havok Content Tools is organized as a sequential combination of operators, where the output of each operator is taken as the input of the next one. These individual operators are known as filter's in the Havok Filter Pipeline. There are multiple types of filter: some modify objects; some create new objects based on existing ones; some delete objects. Filters can also perform operations which have no affect on the content, such as previewing the scene or writing it to a file. Havok provides a large set of filters, which are individually described later in this chapter. One of the advantages of this scheme is that the small granularity and the loose coupling between filters makes them particularly easy to write and extend. The Writing Your Own Filter section at the end of this document gives more details on how to do so.

Before looking at the individual filters, lets take a look at the filter manager. This is the application which allows the user to select, organize and modify the options of one or more sets of filters. The filter manager is therefore the "control center" from which asset processing is managed.

1.1. The Filter Manager Interface
The following interface is presented to the user after running any of the Havok Scene Exporters (or from the standalone filter manager, described in the following section):


The interface is divided into several panes:

Available Filters: Shows a list of the available filters, grouped into the following categories :

Core: Filters that operate on generic scene objects (are not related to either physics or animation).

Physics: Filters that create / operate on physics objects (rigid bodies, constraints, etc).

Animation: Filters that create / operate on animation objects (skeletons, skin, motion, etc).

Graphics: Filters related to graphics elements (materials, etc). Also contains the scene previewer.

User: This category is left open for custom filters written by users.

Configuration Set: Lists the set of configurations and the filter setup for the currently selected configuration. Filters are executed in the order that they appear in the list.

Filter Options: Many filters have options which affect the filter's behavior. Whenever a filter is selected in the filter configuration list, any options it may have shown in this pane.

Log Window: This is used to report messages, warnings, etc. while setting up the filter configurations and while processing. Any serious messages are reported in an orange/red color - if any such messages occur they should be investigated further.

There are also some buttons in the filter manager UI:

Add >>: Adds the filter currently selected in the Available Filters list to the current filters list. Double clicking on a filter performs the same operation.

<< Remove: Removes the current filter selected in the list. Pressing DEL performs the same operation.

Move Up: Moves the currently selected filter in the list one slot up. Pressing the "-" key performs the same operation.

Move Down: Moves the currently selected filter in the list one slot down. Pressing the "+" key performs the same operation.

Run Configuration: Executes the current configuration.

Run All Configurations: Executes all configurations consecutively.

Close: Saves all changes to the configurations and closes the filter manager.

Cancel: Closes the filter manager, prompting for confirmation to save any changes made to the configuration(s). Pressing ESCAPE, or closing the window performs the same operation.

1.1.1. Configuration Sets
A set of filters in a specific order and with specific options is known as a configuration in the Havok Filter Pipeline. It is possible to have multiple filter configurations associated with a single asset. The Configuration Set pane contains a dropdown list showing the active configuration, of which there is always at least one by default:


Each configuration contains its own set of filters, and operates on its own copy of the asset data. This allows the same original asset data to be processed in different ways without having to repeatedly change the filters being used. For example, multiple configurations can be used to process animation data using different amounts or types of compression.

To change the active configuration, simply select another from the dropdown list. To rename a configuration, edit it's name using the box. To delete a configuration, press the Del button. To add a new configuration, press the New button.

Whenever a new configuration is added, it becomes the active configuration, and begins as a copy the previous configuration.

1.1.1.1. Saving & Loading Configuration Sets
When invoked from a modeler, the filter manager configurations are automatically saved as part of the modeler asset file. In addition, configuration sets can be saved and loaded into a standalone file (also called an "options file", or .hko file), using the File > Save Configuration Set and File > Load Configuration Set menu options.

This allows the same filter setups to be used again and again, or tweaked as desired and resaved. They can even be used to override or upgrade the filters saved with individual assets. See the section on Overriding and Upgrading Filter Settings for more information on how to do this.

Additionally, commonly used configurations have been provided for various Havok products and can be loaded using the File > Preset Configurations > menu options:


1.1.2. Product Selection
Filters produce output that may depend on different Havok SDK products to allow them to be loaded at run-time. For example: Scene Transform works for all Havok products; Create Rigid Bodies generates objects which require the Havok Physics/Complete SDK for loading at run-time; Create Rag Doll filter generates objects which require the Havok Complete (Physics+Animation) SDK for loading at run-time.

All filters are always available for use within the filter pipeline, regardless of the run-time product which your company has licensed. Therefore, it is useful to be aware of which filters may be unsupported by your Havok product. You can select your specific Havok product by using the Product menu in the filter manager:


This product selection is stored in the registry and therefore is a user-setting - its value remains between each session of the filter manager. Filters which are not supported at run-time by the selected product will appear in red in the filter manager, indicating that they should not be used:


On execution of any setup which includes run-time unsupported filters, a warning will appear in the log window for each unsupported filter.

1.2. The Standalone Filter Manager
The filter manager is implemented as a DLL - each modeler invokes the same filter manager through the DLL interface. It is also possible to invoke the filter manager through a standalone Havok application, known as the the Standalone Filter Manager.

The input data for this application is taken from one or multiple Havok serialized (.hkx) files. These files are usually the output of some processing previously performed from a filter manager invoked from a modeler.

The standalone filter manager has many possible uses:

It can be used as a "preview" tool (by loading an asset and just using the Preview Scene filter).

It can be used to batch process assets without the need to use a modeler.

1.2.1. Using the Standalone Filter Manager in Interactive Mode
To use the Standalone Filter Manager in interactive mode simply launch the associated executable (a shortcut is placed in the Start menu by the Havok Content Tools installer). A splash screen will appear for a short period and you will be presented with a file open dialog box.


Select a single file or multiple files to load and press the Load button. Each time you press the Load button the title in the dialog is updated to reflect the number of files selected for loading. Once you have successfully selected one or more files press the Done button to complete the loading process.

The files are loaded in turn using our standard serialization. If any file fails to load a warning is printed in the console window.

Successfully loaded files are then merged together into a single root level container and this is passed to the filter manager. The filter manager should now appear and interactively allow you to adjust the current filter stack.

1.2.1.1. Default Options
When .hkx files are loaded into the Standalone Filter Manager there are no filter configurations available, so a default configuration set is used instead. This configuration, called 'HKX Preview', contains a single Preview Scene filter, allowing the user to view to .hkx file contents.

This default setup is stored in an XML file called "defaults.hko", placed in the same folder as the Standalone Filter Manager. This allows the default options to be modified or replaced as necessary. See the HKO Files section for more information on creating or modifying options.

Once loaded by the Standalone Filter Manager, the filter configurations can be edited and/or processed as per usual.

1.2.2. Command Line Arguments
The Standalone Filter Manager can also be launched in command line mode. It's usage is as follows:

 USAGE: hkStandaloneFilterManager.exe [-p assetPath -s
        settingFilename.hko -o outputPath] file1.hkx [file2.hkx ..
        fileN.hkx]
This launches the Standalone Filter Manager in command line mode. The specified files (file1..N) are loaded and merged before being passed to the filter manager. The filter manager is automatically invoked and processing begins in batch mode. If you wish to specify files on the command line and still run the previewer in interactive mode, you can use the -i flag to force interactive mode.

The optional -s flag specifies a filter set (i.e. a .hko file) to use. .hko files are usually created by launching the tool interactively, creating one or more filter stacks and choosing the 'save filter set' option. If this flag is omitted the default options file will be used.

The optional -p flag specifies the asset path, described further in the following section.

The optional -o flag specifies the output path. If not specified, the current working directory is used.

The main reason for using the command line mode is to efficiently process assets and create .hkx files. To this end, the filter set should contain a Platform Writer filter.

1.2.3. Asset Paths
When .hkx files are created using relative paths in the filters, they are relative to the asset path. Thus if you load several files you have several asset paths to choose from, or if you have an .hkx file that should not use its asset path for this run, then you can specify your path. The asset path dialog pops up after you have chosen your file set with the list of asset paths contained in those files. You can choose one of them, edit them, or just browse for one if you can't remember it. If you choose Cancel, the pipeline will run with a empty asset path, so any relative paths will be in relation to the current working directory.


1.2.4. Examples
The Standalone Filter Manager was originally designed to allow you to view .hkx files without having to resort to loading the original art asset in the modeler. However, since it uses the full power of the filter manager it can also be used directly in the tool chain as an asset processing tool. Some use cases would be:

Use a View Xml filter to verify that the expected file elements are present in the asset.

Load separate rigs, animation and skin data to verify they work together.

Use any of the motion extraction or compression filters on raw animation to test their results in isolation.

Batch process assets to transform a scene from a left handed to right handed system.

Batch process XML production assets to produce final game assets in binary platform specific formats.
