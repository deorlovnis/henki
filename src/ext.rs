//! Extensions: directories that teach the model a CLI. `NAME/doc.md`'s first line goes into the prompt; an optional
//! `NAME/bin/` goes first on the tool PATH.
use std::path::PathBuf;

pub struct Ext {
    pub name: String,
    pub line: String,
    pub doc: PathBuf,
    pub bin: Option<PathBuf>,
    pub cmds: Vec<String>,
    pub shadows: Vec<PathBuf>,
    pub paths: Vec<PathBuf>,
    pub targets: Vec<PathBuf>,
}
