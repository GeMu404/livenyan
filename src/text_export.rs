//! Exporta diálogos y subtítulos traducibles para preparar catálogos.

use crate::archive::GameData;
use crate::lsb::{CmdType, Field, Lsb, Param, names::Func};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Catalog {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    entries: Vec<CatalogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CatalogEntry {
    id: String,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportSummary {
    pub entries: usize,
    pub dialogues: usize,
    pub menu_items: usize,
    pub movies: usize,
}

#[derive(Debug, Clone)]
pub struct LanguageCatalog {
    pub code: String,
    pub name: String,
    pub translations: HashMap<String, String>,
}

pub fn load_languages(directory: &Path) -> Result<Vec<LanguageCatalog>, String> {
    let files = match std::fs::read_dir(directory) {
        Ok(files) => files,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read {}: {error}", directory.display())),
    };
    let mut paths = files
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| format!("cannot read entry in {}: {error}", directory.display()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.retain(|path| {
        path.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
            && !path
                .file_stem()
                .is_some_and(|stem| stem.eq_ignore_ascii_case("texts"))
    });
    paths.sort();

    paths
        .into_iter()
        .map(|path| {
            let contents = std::fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            let catalog: Catalog = toml::from_str(&contents)
                .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
            if catalog.version != 1 {
                return Err(format!(
                    "unsupported translation catalog version {} in {}",
                    catalog.version,
                    path.display()
                ));
            }
            let translations: HashMap<String, String> = catalog
                .entries
                .into_iter()
                .filter_map(|entry| {
                    if !is_runtime_translation_id(&entry.id) {
                        return None;
                    }
                    (!entry.text.is_empty()).then_some((entry.id, entry.text))
                })
                .collect();
            let Some(code) = path.file_stem().and_then(|stem| stem.to_str()) else {
                return Err(format!("invalid translation filename {}", path.display()));
            };
            Ok((code.to_string(), catalog.name, translations))
        })
        .filter_map(|result| match result {
            Ok((_, _, translations)) if translations.is_empty() => None,
            Ok((code, name, translations)) => Some(Ok(LanguageCatalog {
                name: name.unwrap_or_else(|| code.clone()),
                code,
                translations,
            })),
            Err(error) => Some(Err(error)),
        })
        .collect()
}

pub fn export(data: &GameData, directory: &Path) -> Result<ExportSummary, String> {
    let mut entries = Vec::new();
    let mut dialogues = 0;
    let mut menu_items = 0;
    let mut movies = 0;
    let mut scripts = Vec::new();

    for name in data.names() {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".lsb") {
            let raw = data
                .read(name)
                .map_err(|error| format!("cannot read {name}: {error}"))?;
            let script =
                Lsb::parse(name, &raw).map_err(|error| format!("cannot parse {name}: {error}"))?;
            scripts.push(script);
        } else if lower.ends_with(".lcm") {
            let raw = data
                .read(name)
                .map_err(|error| format!("cannot read {name}: {error}"))?;
            let movie =
                crate::lcm::parse(&raw).map_err(|error| format!("cannot parse {name}: {error}"))?;
            for (clip_index, clip) in movie.clips.iter().enumerate() {
                if let crate::lcm::TrackContent::Text { value, .. } = &clip.content {
                    push_entry(
                        &mut entries,
                        format!("{name}:clip={clip_index}"),
                        value.clone(),
                    );
                }
            }
            movies += 1;
        }
    }

    let global_strings = collect_static_string_variables(&scripts);
    for script in &scripts {
        for (command_index, command) in script.commands.iter().enumerate() {
            if CmdType::from(command.kind) == CmdType::TextIns
                && let Some(text) = command.text()
            {
                let id = format!(
                    "{}:line={}:command={command_index}:field=Text",
                    script.name, command.line_no
                );
                if push_entry(&mut entries, id, text.translation_template()) {
                    dialogues += 1;
                }
            }
        }
        menu_items += collect_menu_item_labels(script, &global_strings, &mut entries);
    }

    let path = directory.join("texts.toml");
    if path.exists() {
        let previous = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let previous: Catalog = toml::from_str(&previous)
            .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
        let translations: HashMap<String, String> = previous
            .entries
            .into_iter()
            .map(|entry| (entry.id, entry.text))
            .collect();
        for entry in &mut entries {
            if let Some(text) = translations.get(&entry.id) {
                entry.text.clone_from(text);
            }
        }
    }

    std::fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let catalog = Catalog {
        version: 1,
        name: None,
        entries,
    };
    let output = toml::to_string_pretty(&catalog)
        .map_err(|error| format!("cannot serialize translation catalog: {error}"))?;
    std::fs::write(&path, output)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;

    Ok(ExportSummary {
        entries: catalog.entries.len(),
        dialogues,
        menu_items,
        movies,
    })
}

fn is_runtime_translation_id(id: &str) -> bool {
    if let Some((path, clip)) = id.rsplit_once(":clip=") {
        return path.to_ascii_lowercase().ends_with(".lcm") && clip.parse::<usize>().is_ok();
    }
    if let Some(encoded) = id.strip_prefix("menu:item=") {
        return !encoded.is_empty()
            && encoded.len().is_multiple_of(2)
            && encoded.bytes().all(|byte| byte.is_ascii_hexdigit());
    }
    let Some((prefix, field)) = id.rsplit_once(":field=") else {
        return false;
    };
    if field != "Text" {
        return false;
    }
    let Some((prefix, command)) = prefix.rsplit_once(":command=") else {
        return false;
    };
    if command.parse::<usize>().is_err() {
        return false;
    }
    let Some((script, line)) = prefix.rsplit_once(":line=") else {
        return false;
    };
    !script.is_empty()
        && script.to_ascii_lowercase().ends_with(".lsb")
        && line.parse::<u32>().is_ok()
}

pub fn menu_item_id(source: &str) -> String {
    let encoded = source
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("menu:item={encoded}")
}

fn collect_menu_item_labels(
    script: &Lsb,
    global_strings: &HashMap<String, Vec<String>>,
    entries: &mut Vec<CatalogEntry>,
) -> usize {
    let mut count = 0;
    let (displayed_arrays, call_menu_arrays) = menu_source_arrays(script);
    let mut menu_arrays = displayed_arrays;
    menu_arrays.extend(call_menu_arrays);
    let mut static_strings = HashMap::<String, Vec<String>>::new();
    for command in &script.commands {
        for expression in command_expressions(command) {
            let mut temporaries = HashMap::<String, String>::new();
            for operation in &expression.entries {
                if operation.func == Some(Func::ADDARRAY)
                    && operation.operands.len() >= 2
                    && matches!(operation.operands.first(), Some(Param::Var(name)) if menu_arrays.contains(name))
                {
                    if let Some(label) = resolve_static_strings(
                        operation.operands.get(1),
                        &temporaries,
                        &static_strings,
                        global_strings,
                    ) {
                        for label in label {
                            count +=
                                push_entry(entries, menu_item_id(&label), label.clone()) as usize;
                        }
                    }
                } else if operation.func == Some(Func::STRINGTOARRAY)
                    && operation.operands.len() >= 2
                    && matches!(operation.operands.get(1), Some(Param::Var(name)) if menu_arrays.contains(name))
                    && let Some(sources) = resolve_static_strings(
                        operation.operands.first(),
                        &temporaries,
                        &static_strings,
                        global_strings,
                    )
                {
                    let separators = resolve_static_strings(
                        operation.operands.get(2),
                        &temporaries,
                        &static_strings,
                        global_strings,
                    )
                    .unwrap_or_else(|| vec!["\r\n".to_string()]);
                    for source in sources {
                        for label in source
                            .split(separators.first().map(String::as_str).unwrap_or("\r\n"))
                            .filter(|label| !label.is_empty())
                        {
                            count += push_entry(entries, menu_item_id(label), label.to_string())
                                as usize;
                        }
                    }
                }

                if operation.op == crate::lsb::op::TO {
                    if let Some(values) = resolve_static_strings(
                        operation.operands.last(),
                        &temporaries,
                        &static_strings,
                        global_strings,
                    ) {
                        if operation.name.starts_with("____") {
                            temporaries.insert(
                                operation.name.clone(),
                                values.into_iter().next().unwrap_or_default(),
                            );
                        } else {
                            static_strings.insert(operation.name.clone(), values);
                        }
                    }
                }
            }
        }
    }
    count
}

fn menu_source_arrays(script: &Lsb) -> (HashSet<String>, HashSet<String>) {
    let mut dependencies = HashMap::<String, HashSet<String>>::new();
    let mut displayed = HashSet::new();
    let mut call_arrays = HashSet::new();
    for command in &script.commands {
        for (_, field) in &command.fields {
            let crate::lsb::Field::Props(properties) = field else {
                continue;
            };
            if !matches!(
                CmdType::from(command.kind),
                CmdType::Menu | CmdType::Caption | CmdType::CGCaption
            ) {
                continue;
            }
            for (property, expression) in properties {
                if *property == crate::lsb::Prop::TEXT {
                    displayed.extend(expression_dependencies(expression, &mut dependencies));
                }
            }
        }
        for expression in command_expressions(command) {
            for (operation, _) in expression_operation_dependencies(expression, &mut dependencies) {
                if CmdType::from(command.kind) == CmdType::Call
                    && operation.func == Some(Func::ARRAYTOSTRING)
                    && let Some(Param::Var(array)) = operation.operands.first()
                {
                    call_arrays.insert(array.clone());
                }
            }
        }
    }
    let mut pending = displayed.iter().cloned().collect::<Vec<_>>();
    while let Some(variable) = pending.pop() {
        if let Some(references) = dependencies.get(&variable) {
            for reference in references {
                if displayed.insert(reference.clone()) {
                    pending.push(reference.clone());
                }
            }
        }
    }
    (displayed, call_arrays)
}

fn expression_dependencies(
    expression: &crate::lsb::LiveParser,
    dependencies: &mut HashMap<String, HashSet<String>>,
) -> HashSet<String> {
    expression_operation_dependencies(expression, dependencies)
        .into_iter()
        .flat_map(|(_, references)| references)
        .collect()
}

fn expression_operation_dependencies<'a>(
    expression: &'a crate::lsb::LiveParser,
    dependencies: &mut HashMap<String, HashSet<String>>,
) -> Vec<(&'a crate::lsb::OpeData, HashSet<String>)> {
    let mut temporaries = HashMap::<String, HashSet<String>>::new();
    let mut result = Vec::new();
    for operation in &expression.entries {
        let references = operation
            .operands
            .iter()
            .filter_map(|operand| match operand {
                Param::Var(name) => Some(
                    temporaries
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| HashSet::from([name.clone()])),
                ),
                _ => None,
            })
            .flatten()
            .collect::<HashSet<_>>();
        if operation.name.starts_with("____") {
            temporaries.insert(operation.name.clone(), references.clone());
        } else {
            dependencies
                .entry(operation.name.clone())
                .or_default()
                .extend(references.iter().cloned());
        }
        result.push((operation, references));
    }
    result
}

fn command_expressions(command: &crate::lsb::Command) -> Vec<&crate::lsb::LiveParser> {
    let mut expressions = Vec::new();
    for (_, field) in &command.fields {
        match field {
            crate::lsb::Field::Expr(expression) => expressions.push(expression),
            crate::lsb::Field::Exprs(items) => expressions.extend(items),
            crate::lsb::Field::Props(properties) => {
                expressions.extend(properties.iter().map(|(_, expression)| expression));
            }
            _ => {}
        }
    }
    expressions
}

fn collect_static_string_variables(scripts: &[Lsb]) -> HashMap<String, Vec<String>> {
    let mut variables = HashMap::<String, Vec<String>>::new();
    for script in scripts {
        for command in &script.commands {
            if let Some(Field::Expr(initializer)) = command.field("InitVal")
                && CmdType::from(command.kind) == CmdType::VarNew
                && let Some(crate::lsb::Param::Str(value)) = initializer.literal()
            {
                insert_static_string(
                    &mut variables,
                    command.str("Name").unwrap_or(""),
                    value.clone(),
                );
            }
            for expression in command_expressions(command) {
                let mut temporaries = HashMap::<String, String>::new();
                for operation in &expression.entries {
                    if operation.op != crate::lsb::op::TO {
                        continue;
                    }
                    let value = match operation.operands.last() {
                        Some(Param::Str(value)) => Some(value.clone()),
                        Some(Param::Var(name)) => temporaries.get(name).cloned(),
                        _ => None,
                    };
                    if let Some(value) = value {
                        if operation.name.starts_with("____") {
                            temporaries.insert(operation.name.clone(), value);
                        } else {
                            insert_static_string(&mut variables, &operation.name, value);
                        }
                    }
                }
            }
        }
    }
    variables
}

fn insert_static_string(variables: &mut HashMap<String, Vec<String>>, name: &str, value: String) {
    if !name.is_empty() && !value.is_empty() {
        let values = variables.entry(name.to_string()).or_default();
        if !values.contains(&value) {
            values.push(value);
        }
    }
}

fn resolve_static_strings(
    parameter: Option<&Param>,
    temporaries: &HashMap<String, String>,
    variables: &HashMap<String, Vec<String>>,
    globals: &HashMap<String, Vec<String>>,
) -> Option<Vec<String>> {
    match parameter {
        Some(Param::Str(value)) => Some(vec![value.clone()]),
        Some(Param::Var(name)) => temporaries
            .get(name)
            .map(|value| vec![value.clone()])
            .or_else(|| variables.get(name).cloned())
            .or_else(|| globals.get(name).cloned()),
        Some(Param::Int(_) | Param::Float(_) | Param::Flag(_)) | None => None,
    }
}

fn push_entry(entries: &mut Vec<CatalogEntry>, id: String, text: String) -> bool {
    if !text.is_empty() {
        if entries.iter().any(|entry| entry.id == id) {
            return false;
        }
        entries.push(CatalogEntry { id, text });
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Catalog, collect_menu_item_labels, collect_static_string_variables, export,
        is_runtime_translation_id, menu_item_id,
    };
    use crate::archive::GameData;
    use crate::lsb::{CmdType, Command, Field, LiveParser, Lsb, OpeData, Param, names::Func, op};
    use std::path::PathBuf;

    fn script(commands: Vec<Command>) -> Lsb {
        Lsb {
            name: "fixture.lsb".into(),
            version: 117,
            flags: 0,
            command_params: Vec::new(),
            commands,
        }
    }

    fn calc(expression: LiveParser) -> Command {
        Command {
            kind: CmdType::Calc as u8,
            indent: 0,
            mute: false,
            not_update: false,
            line_no: 1,
            fields: vec![("Calc", Field::Expr(expression))],
        }
    }

    fn menu_text(array: &str) -> Command {
        Command {
            kind: CmdType::Menu as u8,
            indent: 0,
            mute: false,
            not_update: false,
            line_no: 2,
            fields: vec![(
                "components",
                Field::Props(vec![(
                    crate::lsb::Prop::TEXT,
                    LiveParser {
                        entries: vec![OpeData {
                            op: op::TO,
                            name: "____0".into(),
                            func: None,
                            operands: vec![Param::Var(array.into())],
                        }],
                    },
                )]),
            )],
        }
    }

    fn call_array(array: &str) -> Command {
        Command {
            kind: CmdType::Call as u8,
            indent: 0,
            mute: false,
            not_update: false,
            line_no: 3,
            fields: vec![(
                "Params",
                Field::Exprs(vec![LiveParser {
                    entries: vec![OpeData {
                        op: op::FUNC,
                        name: "____0".into(),
                        func: Some(Func::ARRAYTOSTRING),
                        operands: vec![Param::Var(array.into())],
                    }],
                }]),
            )],
        }
    }

    fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    #[test]
    fn exports_catalog_and_preserves_edited_text() {
        let data = GameData::open(&fixture_dir().join("game.exe")).unwrap();
        let directory = std::env::temp_dir().join(format!(
            "livenyan-text-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("texts.toml");

        let first = export(&data, &directory).unwrap();
        assert!(first.entries > 0);
        assert!(first.dialogues > 0);
        let mut catalog: Catalog =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(catalog.entries.iter().all(|entry| !entry.id.is_empty()));
        assert!(
            catalog
                .entries
                .iter()
                .all(|entry| super::is_runtime_translation_id(&entry.id)),
            "catalog contains non-dialogue text"
        );
        catalog.entries[0].text = "translated".to_string();
        std::fs::write(&path, toml::to_string(&catalog).unwrap()).unwrap();

        export(&data, &directory).unwrap();
        let catalog: Catalog = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(catalog.entries[0].text, "translated");

        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn discovers_multiple_language_catalogs_and_ignores_empty_templates() {
        let directory = std::env::temp_dir().join(format!(
            "livenyan-language-discovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("es.toml"),
            r#"version = 1
name = "Español"

[[entries]]
id = "script.lsb:line=1:command=0:field=Text"
text = "traducido"
"#,
        )
        .unwrap();
        std::fs::write(
            directory.join("texts.toml"),
            r#"version = 1

[[entries]]
id = "script.lsb:line=2:command=1:field=Text"
source = "texto original"
translation = ""
"#,
        )
        .unwrap();

        let languages = super::load_languages(&directory).unwrap();
        assert_eq!(languages.len(), 1);
        assert_eq!(languages[0].code, "es");
        assert_eq!(languages[0].name, "Español");
        assert_eq!(
            languages[0]
                .translations
                .get("script.lsb:line=1:command=0:field=Text")
                .map(String::as_str),
            Some("traducido")
        );

        std::fs::remove_file(directory.join("es.toml")).unwrap();
        std::fs::remove_file(directory.join("texts.toml")).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn loader_ignores_catalog_entries_that_are_not_runtime_dialogue() {
        let directory = std::env::temp_dir().join(format!(
            "livenyan-language-filter-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("test.toml"),
            r#"version = 1

[[entries]]
id = "script.lsb:line=12:command=3:field=Text"
text = "diálogo"

[[entries]]
id = "script.lsb:line=12:command=3:field=Name"
text = "nombre de objeto"
"#,
        )
        .unwrap();

        let languages = super::load_languages(&directory).unwrap();
        assert_eq!(languages.len(), 1);
        assert_eq!(languages[0].translations.len(), 1);
        assert_eq!(
            languages[0]
                .translations
                .get("script.lsb:line=12:command=3:field=Text")
                .map(String::as_str),
            Some("diálogo")
        );

        std::fs::remove_file(directory.join("test.toml")).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn loader_accepts_menu_item_translations() {
        let directory = std::env::temp_dir().join(format!(
            "livenyan-menu-language-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let id = menu_item_id("Save");
        std::fs::write(
            directory.join("es.toml"),
            format!("version = 1\n\n[[entries]]\nid = {id:?}\ntext = \"Guardar\"\n"),
        )
        .unwrap();

        let languages = super::load_languages(&directory).unwrap();
        assert_eq!(languages.len(), 1);
        assert_eq!(
            languages[0].translations.get(&id).map(String::as_str),
            Some("Guardar")
        );

        std::fs::remove_file(directory.join("es.toml")).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn extracts_static_menu_labels_from_string_to_array() {
        let expression = LiveParser {
            entries: vec![
                OpeData {
                    op: op::TO,
                    name: "____0".into(),
                    func: None,
                    operands: vec![Param::Str(",".into())],
                },
                OpeData {
                    op: op::FUNC,
                    name: "____1".into(),
                    func: Some(Func::STRINGTOARRAY),
                    operands: vec![
                        Param::Str("Save,Options".into()),
                        Param::Var("SystemMenuItemNames".into()),
                        Param::Var("____0".into()),
                    ],
                },
            ],
        };
        let script = script(vec![calc(expression), menu_text("SystemMenuItemNames")]);
        let globals = collect_static_string_variables(std::slice::from_ref(&script));
        let mut entries = Vec::new();
        assert_eq!(collect_menu_item_labels(&script, &globals, &mut entries), 2);
        assert_eq!(entries[0].text, "Save");
        assert_eq!(entries[1].text, "Options");
        assert!(is_runtime_translation_id(&entries[0].id));
        assert_eq!(entries[1].id, menu_item_id("Options"));
    }

    #[test]
    fn extracts_menu_choices_appended_to_the_runtime_menu_array() {
        let expression = LiveParser {
            entries: vec![
                OpeData {
                    op: op::TO,
                    name: "____0".into(),
                    func: None,
                    operands: vec![Param::Var("choice_label".into())],
                },
                OpeData {
                    op: op::FUNC,
                    name: "____1".into(),
                    func: Some(Func::ADDARRAY),
                    operands: vec![Param::Var("_tmp".into()), Param::Var("____0".into())],
                },
            ],
        };
        let globals_script = script(vec![Command {
            kind: CmdType::VarNew as u8,
            indent: 0,
            mute: false,
            not_update: false,
            line_no: 1,
            fields: vec![
                ("Name", Field::Str("choice_label".into())),
                (
                    "InitVal",
                    Field::Expr(LiveParser {
                        entries: vec![OpeData {
                            op: op::TO,
                            name: "____0".into(),
                            func: None,
                            operands: vec![Param::Str("会話モード設定".into())],
                        }],
                    }),
                ),
            ],
        }]);
        let menu_script = script(vec![calc(expression), call_array("_tmp")]);
        let globals = collect_static_string_variables(&[globals_script, menu_script.clone()]);
        let mut entries = Vec::new();
        assert_eq!(
            collect_menu_item_labels(&menu_script, &globals, &mut entries),
            1
        );
        assert_eq!(entries[0].text, "会話モード設定");
        assert_eq!(entries[0].id, menu_item_id("会話モード設定"));
    }
}
