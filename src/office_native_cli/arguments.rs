use a3s_use_core::UseResult;

use super::usage_error;

#[derive(Debug, Default)]
pub(super) struct ParsedArguments {
    pub positionals: Vec<String>,
    pub depth: Option<usize>,
    pub limit: Option<usize>,
    pub timeout_ms: Option<u64>,
    pub text: Option<String>,
    pub find: Option<String>,
    pub replacement: Option<String>,
    pub output: Option<String>,
    pub input: Option<String>,
    pub node_type: Option<String>,
    pub name: Option<String>,
    pub alt: Option<String>,
    pub width: Option<u32>,
    pub width_emu: Option<u64>,
    pub height: Option<u32>,
    pub rows: Option<usize>,
    pub columns: Option<usize>,
    pub number: Option<String>,
    pub boolean: Option<String>,
    pub formula: Option<String>,
    pub bold: Option<String>,
    pub italic: Option<String>,
    pub underline: Option<String>,
    pub script: Option<String>,
    pub strikethrough: Option<String>,
    pub font_family: Option<String>,
    pub font_size: Option<String>,
    pub text_color: Option<String>,
    pub alignment: Option<String>,
    pub url: Option<String>,
    pub location: Option<String>,
    pub display: Option<String>,
    pub tooltip: Option<String>,
    pub author: Option<String>,
    pub initials: Option<String>,
    pub x_emu: Option<i32>,
    pub y_emu: Option<i32>,
    pub count: Option<u32>,
    pub position: Option<usize>,
    pub index: Option<usize>,
    pub target_parent: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub data: Option<String>,
    pub force: bool,
    pub regex: bool,
}

impl ParsedArguments {
    pub fn parse(args: &[String], allowed: AllowedOptions) -> UseResult<Self> {
        let mut parsed = Self::default();
        let mut index = 1;
        while index < args.len() {
            match args[index].as_str() {
                "--json" => index += 1,
                "--" => {
                    parsed.positionals.extend_from_slice(&args[index + 1..]);
                    break;
                }
                "--depth" if allowed.depth => {
                    if parsed.depth.is_some() {
                        return Err(usage_error("--depth may be specified only once"));
                    }
                    let value = option_value(args, index, "--depth")?;
                    parsed.depth = Some(value.parse::<usize>().map_err(|_| {
                        usage_error(format!(
                            "--depth requires a non-negative integer, received '{value}'"
                        ))
                    })?);
                    index += 2;
                }
                "--limit" if allowed.limit => {
                    set_usize_option(&mut parsed.limit, args, index, "--limit")?;
                    index += 2;
                }
                "--timeout-ms" if allowed.timeout_ms => {
                    set_u64_option(&mut parsed.timeout_ms, args, index, "--timeout-ms")?;
                    index += 2;
                }
                "--text" if allowed.text => {
                    set_string_option(&mut parsed.text, args, index, "--text")?;
                    index += 2;
                }
                "--find" if allowed.find => {
                    set_string_option(&mut parsed.find, args, index, "--find")?;
                    index += 2;
                }
                "--replace" if allowed.replacement => {
                    set_string_option(&mut parsed.replacement, args, index, "--replace")?;
                    index += 2;
                }
                "--number" if allowed.number => {
                    set_string_option(&mut parsed.number, args, index, "--number")?;
                    index += 2;
                }
                "--boolean" | "--bool" if allowed.boolean => {
                    set_string_option(&mut parsed.boolean, args, index, "--boolean")?;
                    index += 2;
                }
                "--formula" if allowed.formula => {
                    set_string_option(&mut parsed.formula, args, index, "--formula")?;
                    index += 2;
                }
                "--bold" if allowed.bold => {
                    set_string_option(&mut parsed.bold, args, index, "--bold")?;
                    index += 2;
                }
                "--italic" if allowed.italic => {
                    set_string_option(&mut parsed.italic, args, index, "--italic")?;
                    index += 2;
                }
                "--underline" if allowed.underline => {
                    set_string_option(&mut parsed.underline, args, index, "--underline")?;
                    index += 2;
                }
                "--script" if allowed.script => {
                    set_string_option(&mut parsed.script, args, index, "--script")?;
                    index += 2;
                }
                "--strikethrough" if allowed.strikethrough => {
                    set_string_option(&mut parsed.strikethrough, args, index, "--strikethrough")?;
                    index += 2;
                }
                "--font-family" if allowed.font_family => {
                    set_string_option(&mut parsed.font_family, args, index, "--font-family")?;
                    index += 2;
                }
                "--font-size" if allowed.font_size => {
                    set_string_option(&mut parsed.font_size, args, index, "--font-size")?;
                    index += 2;
                }
                "--text-color" if allowed.text_color => {
                    set_string_option(&mut parsed.text_color, args, index, "--text-color")?;
                    index += 2;
                }
                "--align" | "--alignment" if allowed.alignment => {
                    set_string_option(&mut parsed.alignment, args, index, "--align")?;
                    index += 2;
                }
                "--url" | "--link" | "--href" if allowed.url => {
                    set_string_option(&mut parsed.url, args, index, "--url")?;
                    index += 2;
                }
                "--location" if allowed.location => {
                    set_string_option(&mut parsed.location, args, index, "--location")?;
                    index += 2;
                }
                "--display" if allowed.display => {
                    set_string_option(&mut parsed.display, args, index, "--display")?;
                    index += 2;
                }
                "--tooltip" if allowed.tooltip => {
                    set_string_option(&mut parsed.tooltip, args, index, "--tooltip")?;
                    index += 2;
                }
                "--author" if allowed.author => {
                    set_string_option(&mut parsed.author, args, index, "--author")?;
                    index += 2;
                }
                "--initials" if allowed.initials => {
                    set_string_option(&mut parsed.initials, args, index, "--initials")?;
                    index += 2;
                }
                "--x-emu" if allowed.x_emu => {
                    set_i32_option(&mut parsed.x_emu, args, index, "--x-emu")?;
                    index += 2;
                }
                "--y-emu" if allowed.y_emu => {
                    set_i32_option(&mut parsed.y_emu, args, index, "--y-emu")?;
                    index += 2;
                }
                "--output" if allowed.output => {
                    set_string_option(&mut parsed.output, args, index, "--output")?;
                    index += 2;
                }
                "--input" if allowed.input => {
                    set_string_option(&mut parsed.input, args, index, "--input")?;
                    index += 2;
                }
                "--type" if allowed.node_type => {
                    set_string_option(&mut parsed.node_type, args, index, "--type")?;
                    index += 2;
                }
                "--name" if allowed.name => {
                    set_string_option(&mut parsed.name, args, index, "--name")?;
                    index += 2;
                }
                "--alt" if allowed.alt => {
                    set_string_option(&mut parsed.alt, args, index, "--alt")?;
                    index += 2;
                }
                "--width" if allowed.width => {
                    set_u32_option(&mut parsed.width, args, index, "--width")?;
                    index += 2;
                }
                "--width-emu" if allowed.width_emu => {
                    set_u64_option(&mut parsed.width_emu, args, index, "--width-emu")?;
                    index += 2;
                }
                "--height" if allowed.height => {
                    set_u32_option(&mut parsed.height, args, index, "--height")?;
                    index += 2;
                }
                "--rows" if allowed.rows => {
                    set_usize_option(&mut parsed.rows, args, index, "--rows")?;
                    index += 2;
                }
                "--columns" | "--cols" if allowed.columns => {
                    set_usize_option(&mut parsed.columns, args, index, "--columns")?;
                    index += 2;
                }
                "--count" if allowed.count => {
                    if parsed.count.is_some() {
                        return Err(usage_error("--count may be specified only once"));
                    }
                    let value = option_value(args, index, "--count")?;
                    parsed.count = Some(value.parse::<u32>().map_err(|_| {
                        usage_error(format!(
                            "--count requires a non-negative integer, received '{value}'"
                        ))
                    })?);
                    index += 2;
                }
                "--position" if allowed.position => {
                    set_usize_option(&mut parsed.position, args, index, "--position")?;
                    index += 2;
                }
                "--index" if allowed.index => {
                    set_usize_option(&mut parsed.index, args, index, "--index")?;
                    index += 2;
                }
                "--to" if allowed.target_parent => {
                    set_string_option(&mut parsed.target_parent, args, index, "--to")?;
                    index += 2;
                }
                "--before" if allowed.before => {
                    set_string_option(&mut parsed.before, args, index, "--before")?;
                    index += 2;
                }
                "--after" if allowed.after => {
                    set_string_option(&mut parsed.after, args, index, "--after")?;
                    index += 2;
                }
                "--data" if allowed.data => {
                    set_string_option(&mut parsed.data, args, index, "--data")?;
                    index += 2;
                }
                "--force" if allowed.force => {
                    if parsed.force {
                        return Err(usage_error("--force may be specified only once"));
                    }
                    parsed.force = true;
                    index += 1;
                }
                "--regex" if allowed.regex => {
                    if parsed.regex {
                        return Err(usage_error("--regex may be specified only once"));
                    }
                    parsed.regex = true;
                    index += 1;
                }
                option if option.starts_with('-') => {
                    return Err(usage_error(format!(
                        "unknown native Office option '{option}'"
                    )));
                }
                value => {
                    parsed.positionals.push(value.to_string());
                    index += 1;
                }
            }
        }
        Ok(parsed)
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct AllowedOptions {
    depth: bool,
    limit: bool,
    timeout_ms: bool,
    text: bool,
    find: bool,
    replacement: bool,
    output: bool,
    input: bool,
    node_type: bool,
    name: bool,
    alt: bool,
    width: bool,
    width_emu: bool,
    height: bool,
    rows: bool,
    columns: bool,
    number: bool,
    boolean: bool,
    formula: bool,
    bold: bool,
    italic: bool,
    underline: bool,
    script: bool,
    strikethrough: bool,
    font_family: bool,
    font_size: bool,
    text_color: bool,
    alignment: bool,
    url: bool,
    location: bool,
    display: bool,
    tooltip: bool,
    author: bool,
    initials: bool,
    x_emu: bool,
    y_emu: bool,
    count: bool,
    position: bool,
    index: bool,
    target_parent: bool,
    before: bool,
    after: bool,
    data: bool,
    force: bool,
    regex: bool,
}

impl AllowedOptions {
    pub const NONE: Self = Self {
        depth: false,
        limit: false,
        timeout_ms: false,
        text: false,
        find: false,
        replacement: false,
        output: false,
        input: false,
        node_type: false,
        name: false,
        alt: false,
        width: false,
        width_emu: false,
        height: false,
        rows: false,
        columns: false,
        number: false,
        boolean: false,
        formula: false,
        bold: false,
        italic: false,
        underline: false,
        script: false,
        strikethrough: false,
        font_family: false,
        font_size: false,
        text_color: false,
        alignment: false,
        url: false,
        location: false,
        display: false,
        tooltip: false,
        author: false,
        initials: false,
        x_emu: false,
        y_emu: false,
        count: false,
        position: false,
        index: false,
        target_parent: false,
        before: false,
        after: false,
        data: false,
        force: false,
        regex: false,
    };
    pub const GET: Self = Self {
        depth: true,
        ..Self::NONE
    };
    pub const VIEW: Self = Self {
        output: true,
        timeout_ms: true,
        node_type: true,
        limit: true,
        ..Self::NONE
    };
    pub const SET: Self = Self {
        text: true,
        find: true,
        replacement: true,
        output: true,
        number: true,
        boolean: true,
        formula: true,
        bold: true,
        italic: true,
        underline: true,
        script: true,
        strikethrough: true,
        font_family: true,
        font_size: true,
        text_color: true,
        alignment: true,
        url: true,
        location: true,
        display: true,
        tooltip: true,
        author: true,
        initials: true,
        x_emu: true,
        y_emu: true,
        width_emu: true,
        regex: true,
        ..Self::NONE
    };
    pub const BATCH: Self = Self {
        output: true,
        input: true,
        ..Self::NONE
    };
    pub const RAW: Self = Self {
        output: true,
        ..Self::NONE
    };
    pub const DUMP: Self = Self {
        output: true,
        ..Self::NONE
    };
    pub const RAW_SET: Self = Self {
        output: true,
        input: true,
        ..Self::NONE
    };
    pub const ADD: Self = Self {
        text: true,
        output: true,
        input: true,
        node_type: true,
        name: true,
        alt: true,
        width: true,
        height: true,
        rows: true,
        columns: true,
        index: true,
        url: true,
        location: true,
        display: true,
        tooltip: true,
        author: true,
        initials: true,
        x_emu: true,
        y_emu: true,
        ..Self::NONE
    };
    pub const ADD_PART: Self = Self {
        output: true,
        node_type: true,
        ..Self::NONE
    };
    pub const MUTATE: Self = Self {
        output: true,
        ..Self::NONE
    };
    pub const STRUCTURE: Self = Self {
        output: true,
        count: true,
        ..Self::NONE
    };
    pub const COPY: Self = Self {
        output: true,
        position: true,
        ..Self::NONE
    };
    pub const MOVE_NODE: Self = Self {
        output: true,
        index: true,
        target_parent: true,
        before: true,
        after: true,
        ..Self::NONE
    };
    pub const COPY_NODE: Self = Self {
        output: true,
        name: true,
        index: true,
        target_parent: true,
        before: true,
        after: true,
        ..Self::NONE
    };
    pub const MERGE: Self = Self {
        data: true,
        force: true,
        ..Self::NONE
    };
}

pub(super) fn parse_boolean_option(value: &str) -> UseResult<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(usage_error(format!(
            "--boolean requires true or false, received '{value}'"
        ))),
    }
}

fn option_value<'a>(args: &'a [String], index: usize, option: &str) -> UseResult<&'a str> {
    args.get(index + 1)
        .map(String::as_str)
        .ok_or_else(|| usage_error(format!("{option} requires a value")))
}

fn set_string_option(
    target: &mut Option<String>,
    args: &[String],
    index: usize,
    option: &str,
) -> UseResult<()> {
    if target.is_some() {
        return Err(usage_error(format!("{option} may be specified only once")));
    }
    *target = Some(option_value(args, index, option)?.to_string());
    Ok(())
}

fn set_usize_option(
    target: &mut Option<usize>,
    args: &[String],
    index: usize,
    option: &str,
) -> UseResult<()> {
    if target.is_some() {
        return Err(usage_error(format!("{option} may be specified only once")));
    }
    let value = option_value(args, index, option)?;
    *target = Some(value.parse::<usize>().map_err(|_| {
        usage_error(format!(
            "{option} requires a non-negative integer, received '{value}'"
        ))
    })?);
    Ok(())
}

fn set_u32_option(
    target: &mut Option<u32>,
    args: &[String],
    index: usize,
    option: &str,
) -> UseResult<()> {
    if target.is_some() {
        return Err(usage_error(format!("{option} may be specified only once")));
    }
    let value = option_value(args, index, option)?;
    *target = Some(value.parse::<u32>().map_err(|_| {
        usage_error(format!(
            "{option} requires a non-negative integer, received '{value}'"
        ))
    })?);
    Ok(())
}

fn set_u64_option(
    target: &mut Option<u64>,
    args: &[String],
    index: usize,
    option: &str,
) -> UseResult<()> {
    if target.is_some() {
        return Err(usage_error(format!("{option} may be specified only once")));
    }
    let value = option_value(args, index, option)?;
    *target = Some(value.parse::<u64>().map_err(|_| {
        usage_error(format!(
            "{option} requires a non-negative integer, received '{value}'"
        ))
    })?);
    Ok(())
}

fn set_i32_option(
    target: &mut Option<i32>,
    args: &[String],
    index: usize,
    option: &str,
) -> UseResult<()> {
    if target.is_some() {
        return Err(usage_error(format!("{option} may be specified only once")));
    }
    let value = option_value(args, index, option)?;
    *target = Some(value.parse::<i32>().map_err(|_| {
        usage_error(format!(
            "{option} requires a signed 32-bit integer, received '{value}'"
        ))
    })?);
    Ok(())
}
