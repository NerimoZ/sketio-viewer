use std::{
    borrow::Cow,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    document::{Document, reserved_windows_name},
    rich_edit::wide,
};
use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

pub fn normalized(source: &str) -> Cow<'_, str> {
    if !source.contains('\r') {
        return Cow::Borrowed(source);
    }
    let mut output = String::with_capacity(source.len());
    let mut rest = source;
    while let Some((line, next)) = rest.split_once('\r') {
        output.push_str(line);
        output.push('\n');
        rest = next.strip_prefix('\n').unwrap_or(next);
    }
    output.push_str(rest);
    Cow::Owned(output)
}

fn ending_at(line: &str) -> &str {
    if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\r') {
        "\r"
    } else if line.ends_with('\n') {
        "\n"
    } else {
        ""
    }
}

fn source_lines(source: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut offset = 0;
    let bytes = source.as_bytes();
    for (index, &byte) in bytes.iter().enumerate() {
        if byte == b'\n' || (byte == b'\r' && bytes.get(index + 1) != Some(&b'\n')) {
            lines.push(&source[offset..=index]);
            offset = index + 1;
        }
    }
    lines.push(&source[offset..]);
    lines
}

fn restore_endings(original: &str, edited: &str) -> String {
    let old = source_lines(original);
    let new: Vec<_> = edited.split('\n').collect();
    let content = |line: &str| line.len() - ending_at(line).len();
    let equal = |a: &&str, b: &&str| &a[..content(a)] == *b;
    let prefix = old
        .iter()
        .zip(&new)
        .take_while(|(a, b)| equal(a, b))
        .count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| equal(a, b))
        .count();
    let preferred = old
        .iter()
        .map(|line| ending_at(line))
        .find(|ending| !ending.is_empty())
        .unwrap_or("\r\n");
    let mut output = String::with_capacity(edited.len());
    for (index, line) in new.iter().enumerate() {
        output.push_str(line);
        if index + 1 == new.len() {
            break;
        }
        let previous = if index >= new.len() - suffix {
            old.get(old.len() - (new.len() - index))
        } else if index < prefix || index - prefix < old.len() - suffix - prefix {
            old.get(index)
        } else {
            None
        };
        let ending = previous
            .map(|line| ending_at(line))
            .filter(|ending| !ending.is_empty())
            .unwrap_or(preferred);
        output.push_str(ending);
    }
    output
}

fn encode(source: &str, encoding: &str) -> Vec<u8> {
    match encoding {
        "UTF-16 LE" => [0xff, 0xfe]
            .into_iter()
            .chain(source.encode_utf16().flat_map(u16::to_le_bytes))
            .collect(),
        "UTF-16 BE" => [0xfe, 0xff]
            .into_iter()
            .chain(source.encode_utf16().flat_map(u16::to_be_bytes))
            .collect(),
        "UTF-8 BOM" => [0xef, 0xbb, 0xbf]
            .into_iter()
            .chain(source.bytes())
            .collect(),
        _ => source.as_bytes().to_vec(),
    }
}

pub fn save(document: &Document, path: &Path, text: &str) -> Result<Document, String> {
    let original = encode(&document.source, document.encoding);
    write(document, path, text, Some(original))
}

pub fn save_as(document: &Document, path: &Path, text: &str) -> Result<Document, String> {
    if path
        .file_name()
        .is_some_and(|name| reserved_windows_name(&name.to_string_lossy()))
    {
        return Err("Choose a regular filename instead of a Windows device name.".into());
    }
    let original = match fs::metadata(path) {
        Ok(meta) if meta.is_file() && meta.len() <= crate::document::DOCUMENT_LIMIT => {
            Some(fs::read(path).map_err(|error| error.to_string())?)
        }
        Ok(_) => {
            return Err("Choose a regular destination file within the document size limit.".into());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
    };
    write(document, path, text, original)
}

fn write(
    document: &Document,
    path: &Path,
    text: &str,
    original: Option<Vec<u8>>,
) -> Result<Document, String> {
    let source = restore_endings(&document.source, text);
    let bytes = encode(&source, document.encoding);
    if bytes.len() as u64 > crate::document::DOCUMENT_LIMIT {
        return Err("The edited file exceeds the 16 MiB document limit.".into());
    }
    let parent = path.parent().ok_or("The document has no parent folder.")?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = parent.join(format!(".sketio-{}-{stamp}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| error.to_string())?;
    let result = (|| -> io::Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        if let Some(original) = &original {
            if fs::metadata(path)?.len() != original.len() as u64 || fs::read(path)? != *original {
                return Err(io::Error::other(
                    "The file changed on disk. Your edits are still open in this tab.",
                ));
            }
            fs::rename(&temp, path)
        } else {
            // Move without MOVEFILE_REPLACE_EXISTING if the destination did not exist.
            let temp = wide(&temp);
            let path = wide(path);
            if unsafe { MoveFileExW(temp.as_ptr(), path.as_ptr(), 0) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(error.to_string());
    }
    Ok(document.at_path(source, path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_preserves_utf16_bom_and_existing_mixed_line_endings() {
        let original = "a\r\nb\rc\n";
        assert_eq!(normalized(original), "a\nb\nc\n");
        assert_eq!(restore_endings(original, "a\nB\nc\n"), "a\r\nB\rc\n");
        assert_eq!(
            restore_endings(original, "a\nnew\nb\nc\n"),
            "a\r\nnew\r\nb\rc\n"
        );
        assert_eq!(encode("Ж", "UTF-16 BE"), [0xfe, 0xff, 0x04, 0x16]);
    }
}
