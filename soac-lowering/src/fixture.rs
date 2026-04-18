#[derive(Default, Clone)]
pub struct FixtureBlock {
    pub name: String,
    pub input: String,
    pub output: String,
    pub seen_separator: bool,
}

pub enum FixtureSection {
    Waiting,
    Block(FixtureBlock),
}

pub fn parse_fixture(contents: &str) -> Result<Vec<FixtureBlock>, String> {
    let mut blocks = Vec::new();
    let mut section = FixtureSection::Waiting;

    let finalize = |blocks: &mut Vec<FixtureBlock>, block: FixtureBlock| -> Result<(), String> {
        if !block.seen_separator {
            return Err(format!(
                "missing `# ==` separator in transform fixture `{}`",
                block.name
            ));
        }
        blocks.push(block);
        Ok(())
    };

    for raw_line in contents.split_inclusive('\n') {
        let mut line = raw_line;
        let has_newline = line.ends_with('\n');
        if has_newline {
            line = &line[..line.len() - 1];
        }
        if line.ends_with('\r') {
            line = &line[..line.len() - 1];
        }

        let trimmed = line.trim_end();
        if trimmed.starts_with("# ")
            && trimmed.get(..2) == Some("# ")
            && trimmed != "# =="
            && trimmed != "# -- pre-bb --"
            && trimmed != "# -- bb --"
        {
            if let FixtureSection::Block(block) =
                std::mem::replace(&mut section, FixtureSection::Waiting)
            {
                finalize(&mut blocks, block)?;
            }

            let name = trimmed[2..].trim().to_string();
            section = FixtureSection::Block(FixtureBlock {
                name,
                ..FixtureBlock::default()
            });
            continue;
        }

        match &mut section {
            FixtureSection::Waiting => {
                if trimmed == "# ==" && line.trim() == "# ==" {
                    return Err("unexpected `# ==` outside of a fixture block".to_string());
                }
                if !trimmed.is_empty() {
                    return Err(format!(
                        "unexpected content outside of transform fixtures: `{}`",
                        line
                    ));
                }
            }
            FixtureSection::Block(block) => {
                if trimmed == "# ==" && line.trim() == "# ==" {
                    if block.seen_separator {
                        return Err(format!(
                            "multiple `# ==` separators found in transform fixture `{}`",
                            block.name
                        ));
                    }
                    block.seen_separator = true;
                } else if block.seen_separator {
                    block.output.push_str(line);
                    if has_newline {
                        block.output.push('\n');
                    }
                } else {
                    block.input.push_str(line);
                    if has_newline {
                        block.input.push('\n');
                    }
                }
            }
        }
    }

    if let FixtureSection::Block(block) = section {
        finalize(&mut blocks, block)?;
    }

    Ok(blocks)
}

pub fn render_fixture(blocks: &[FixtureBlock]) -> String {
    let mut output = String::new();
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        output.push_str("# ");
        output.push_str(&block.name);
        output.push_str("\n\n");
        let input = block.input.trim_matches('\n');
        if !input.is_empty() {
            output.push_str(input);
            output.push('\n');
        }
        output.push('\n');
        output.push_str("# ==\n\n");
        let output_block = block.output.trim_matches('\n');
        if !output_block.is_empty() {
            output.push_str(output_block);
            output.push('\n');
        }
    }
    output
}
