//! Explain local workspace/private-state conflicts before session creation.
use anyhow::{Context, Result, anyhow};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

fn overlaps(path: &Path, protected: &[PathBuf]) -> bool {
    protected
        .iter()
        .any(|private| path.starts_with(private) || private.starts_with(path))
}

// Resolve existing symlinked ancestors even when the proposed folder is new.
fn resolve_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return path.canonicalize().context("cannot access directory");
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("directory has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("enter a directory without trailing . or .."))?;
    Ok(resolve_path(parent)?.join(name))
}

fn prepare_directory(path: &Path, protected: &[PathBuf]) -> Result<PathBuf> {
    let resolved = resolve_path(path)?;
    if overlaps(&resolved, protected) {
        return Err(anyhow!(
            "Choose a folder outside the private S-Code directories and their parents."
        ));
    }
    fs::create_dir_all(&resolved).context("cannot create work folder")?;
    let resolved = resolved.canonicalize()?;
    if !resolved.is_dir() || overlaps(&resolved, protected) {
        return Err(anyhow!("This folder cannot be used as a workspace."));
    }
    Ok(resolved)
}

pub(crate) fn choose(workspace: &str, interactive: bool) -> Result<Option<String>> {
    let path = url::Url::parse(workspace)?
        .to_file_path()
        .map_err(|_| anyhow!("workspace must be a local directory"))?
        .canonicalize()
        .context("workspace directory cannot be accessed")?;
    let protected = s_code_config::local_product_directories()
        .map_err(anyhow::Error::msg)?
        .iter()
        .map(|path| resolve_path(path))
        .collect::<Result<Vec<_>>>()?;
    if !overlaps(&path, &protected) {
        return Ok(Some(workspace.into()));
    }
    if !interactive {
        return Err(anyhow!(
            "The workspace overlaps S-Code private configuration or state. Run S-Code from a separate project folder; keep S_CODE_HOME outside that folder."
        ));
    }
    println!("\nLet's choose a separate work folder.");
    println!(
        "{} overlaps private S-Code files, including credentials.",
        path.display()
    );
    println!("Choose a separate project folder. Your files and connection will not be moved.");
    let suggested = path.join("s-code-workspace");
    let suggested = resolve_path(&suggested)
        .ok()
        .filter(|path| !overlaps(path, &protected));
    loop {
        if let Some(suggested) = &suggested {
            println!(
                "Enter accepts this folder (created if needed): {}",
                suggested.display()
            );
        }
        print!("Work folder (absolute path, or q to cancel): ");
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 || input.trim() == "q" {
            return Ok(None);
        }
        let selected = if input.trim().is_empty() {
            let Some(suggested) = &suggested else {
                continue;
            };
            suggested.clone()
        } else {
            PathBuf::from(input.trim())
        };
        if !selected.is_absolute() {
            println!("Enter an absolute directory path.");
            continue;
        }
        match prepare_directory(&selected, &protected) {
            Ok(path) => {
                println!("Work folder: {}", path.display());
                return url::Url::from_directory_path(path)
                    .map(|url| Some(url.into()))
                    .map_err(|_| anyhow!("cannot represent work folder as a file URL"));
            }
            Err(error) => println!("{error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_home_inside_checkout_requires_a_sibling_work_folder() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let private = root.join(".test-home");
        fs::create_dir(&private).unwrap();
        let protected = vec![private.clone()];
        assert!(prepare_directory(&root, &protected).is_err());
        assert!(prepare_directory(&private.join("nested"), &protected).is_err());
        assert!(!private.join("nested").exists());
        let work = prepare_directory(&root.join("s-code-workspace"), &protected).unwrap();
        assert!(work.is_dir());
        assert!(!overlaps(&work, &protected));
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_and_parent_components_cannot_bypass_private_directory_check() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let private = root.join("private");
        fs::create_dir(&private).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&private, &alias).unwrap();
        let protected = vec![private.clone()];
        assert!(prepare_directory(&alias.join("new"), &protected).is_err());
        assert!(prepare_directory(&alias.join("../private/new"), &protected).is_err());
        assert!(!private.join("new").exists());
    }
}
