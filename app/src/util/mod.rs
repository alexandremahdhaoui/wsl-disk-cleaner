use anyhow::{bail, Context, Result};

pub fn parse_df(output: &str) -> Result<(u64, u64)> {
    let line = output.lines().nth(1).context("reading the df data line")?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    let [size, used] = fields[..] else {
        bail!("parsing df line {line:?}: want two fields");
    };
    let size = size
        .parse()
        .with_context(|| format!("parsing df size {size:?}"))?;
    let used = used
        .parse()
        .with_context(|| format!("parsing df used {used:?}"))?;
    Ok((size, used))
}

pub fn parse_human_size(text: &str) -> Result<u64> {
    let token = text.split_whitespace().next().unwrap_or_default();
    let split = token
        .find(|c: char| c.is_ascii_alphabetic())
        .with_context(|| format!("parsing size {text:?}: no unit"))?;
    let (number, unit) = token.split_at(split);
    let number: f64 = number
        .parse()
        .with_context(|| format!("parsing size {text:?}: bad number"))?;
    let scale: f64 = match unit.to_ascii_uppercase().as_str() {
        "B" => 1.0,
        "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        other => bail!("parsing size {text:?}: unknown unit {other:?}"),
    };
    Ok((number * scale).round() as u64)
}

pub fn powershell_encoded_command(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64(&bytes)
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let sextets = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for (i, sextet) in sextets.iter().enumerate() {
            if i <= chunk.len() {
                out.push(ALPHABET[*sextet as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn df_output_yields_size_and_used_bytes() {
        let out = "    1B-blocks        Used\n1081101176832 31731884032\n";
        assert_eq!(parse_df(out).unwrap(), (1081101176832, 31731884032));
    }

    #[test]
    fn df_output_without_a_data_line_is_an_error() {
        assert!(parse_df("    1B-blocks        Used\n").is_err());
    }

    #[test]
    fn container_sizes_parse_with_their_percent_suffix() {
        assert_eq!(parse_human_size("4.095GB (98%)").unwrap(), 4_095_000_000);
        assert_eq!(parse_human_size("861.7kB (100%)").unwrap(), 861_700);
        assert_eq!(parse_human_size("0B (0%)").unwrap(), 0);
    }

    #[test]
    fn a_size_with_an_unknown_unit_is_an_error() {
        assert!(parse_human_size("3PiB").is_err());
    }

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn powershell_encoded_command_is_utf16_little_endian_base64() {
        assert_eq!(powershell_encoded_command("dir"), "ZABpAHIA");
    }
}
