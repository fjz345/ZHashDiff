# ZHashDiff

![alt text](crates/zhashdiff-gui/img/showcase2.png)
![alt text](crates/zdiff-gui/img/showcase.png)
![alt text](crates/zhashdiff-gui/img/showcase.png)

Tool I created in order to resolve and delete duplicated files with different file names.
It has evolved in to a propper diffing tool

Good reference & aim for this project: https://www.scootersoftware.com/

# Features
* Tree folder diff view
* Detect & remove duplicate files
* Standalone file diffing tool

# Crates
| Crate | Purpose | Type |
| :--- | :--- | :--- |
| [**zcommon**](./crates/zcommon) | Common | Library |
| [**zdiff**](./crates/zdiff) | File-level diff | Library/CLI |
| [**zdiff-gui**](./crates/zdiff-gui) | Egui for Z-Diff | Application |
| [**zhashdiff**](./crates/zhashdiff) | Folder-level diff | Library/CLI |
| [**zhashdiff-gui**](./crates/zhashdiff-gui) | Egui for Z-HashDiff | Application |
---

# Notes
Hobby project for personal use.