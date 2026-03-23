# Unity Debugger for Zed

<img width="988" height="771" alt="screenshot" src="https://github.com/user-attachments/assets/e83685ed-7b9c-4a0c-9e62-2e7301f89f3a" />

This extension allows you to debug Unity projects in Zed. In order to use it you will need to source your own debug adapter.

> [!IMPORTANT]
> While using the debug adapter provided as part of Unity for Visual Studio Code package is technically possible,
> the project's license specifically disallows use with non-Microsoft IDEs.

> [!WARNING]
> Extension has currently only been tested under macOS. It will be published on Zed's Marketplace once
> an alternative permissively-licensed debug adapter becomes available.

## Setup

Clone this repository, install Rust and use the *Install Dev Extension* button in Zed's Extensions tab.

## Project setup

Open the folder housing your Unity project and create a *debug.json* file in *.zed* subdirectory. Alternatively, press *CMD/CTRL+J* and select *Edit debug.json*. Paste the following contents:
```
[{
    "adapter": "unity",
    "label": "Attach to Unity Editor",
    "request": "attach",
    "projectPath": "${ZED_WORKTREE_ROOT}",
    "adapterPath": "/path/to/UnityDebugAdapter.dll"
}]
```

Adapter path should point towards the debug adapter library, the rest can be left as-is. You may launch the debugger by pressing *CMD/CTRL+J*, clicking the *+* icon and selecting *Attach to Unity Editor*.
