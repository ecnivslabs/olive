# Getting Started

Welcome! Olive is designed to be easy to pick up and fast to run. Here's how to get everything set up on your machine.

## Installation

### Linux and macOS

The easiest way to install Olive is with the install script. It downloads the latest `pit` binary (an all-in-one tool) and adds it to the path.

```bash
curl -sSL https://raw.githubusercontent.com/olive-language/olive/master/install.sh | sh
```

### Windows

Head over to the [releases page](https://github.com/olive-language/olive/releases/latest) and download the `pit-windows-x86_64.exe` binary. Rename it to `pit.exe` and add the folder it's in to your system PATH.

### Verify the Install

Open a new terminal and run:

```bash
pit --version
```

If you see a version number, you're ready to go!

## Your First Project

`pit` is used to manage everything from creating projects to running tests. To start a new project, run:

```bash
pit new my_app
cd my_app
```

This creates a simple project structure for you:
- `src/main.liv`: Where your code lives.
- `pit.toml`: Your project's configuration and dependencies.

## Running Your Code

To run your program, just type:

```bash
pit run
```

Olive is designed for speed. The first time you run a project, it compiles your code and caches it. The second time you run it, it starts almost instantly.

## Hello, World!

Open `src/main.liv` in your favorite editor. You'll see a simple hello world:

```python
fn main():
    print("Hello from Olive!")

main()
```

Try changing the message and running `pit run` again.

## The Interactive Shell

If you just want to test a quick snippet of code without creating a project, use the interactive shell:

```bash
pit shell
```

It's a great way to explore the language and the standard library.

## Updating Olive

Olive is constantly being improved. To get the latest features and bug fixes, run:

```bash
pit upgrade
```

## Pods (Package Management)

Olive uses "pods" for dependencies. You can add them to your project easily:

- `pit add pod_name`: Adds a dependency to your `pit.toml`.
- `pit install`: Downloads all dependencies listed in your `pit.toml`.

All your dependencies are stored locally in the `.pit_pods/` folder, keeping your project self-contained.
