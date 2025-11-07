use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::process;

/// Конфигурация приложения
#[derive(Deserialize, Debug)]
struct Config {
    name: String,
    repository: String,
    test_repo_mode: String, // "test" или "remote"
    version: String,
    output_filename: String,
    ascii_tree_mode: bool,
    max_depth: Option<usize>,
}

/// Структуры для парсинга ответов crates.io
#[derive(Debug, Deserialize)]
struct Dependency {
    crate_id: String,
    kind: Option<String>,
    optional: bool,
}

#[derive(Debug, Deserialize)]
struct DependenciesResponse {
    dependencies: Vec<Dependency>,
}

#[derive(Debug, Deserialize)]
struct VersionInfo {
    num: String,
}

#[derive(Debug, Deserialize)]
struct VersionsResponse {
    versions: Vec<VersionInfo>,
}

fn main() {
    // 1) Читаем конфиг
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Использование: {} <config.json>", args[0]);
        process::exit(1);
    }
    let config_path = &args[1];

    let raw = fs::read_to_string(config_path).unwrap_or_else(|e| {
        eprintln!("Ошибка чтения конфигурации '{}': {}", config_path, e);
        process::exit(1);
    });

    let config: Config = serde_json::from_str(&raw).unwrap_or_else(|e| {
        eprintln!("Ошибка разбора JSON: {}", e);
        process::exit(1);
    });

    // 2) В зависимости от режима строим полный транзитивный граф
    let graph = if config.test_repo_mode == "test" {
        // Тестовый режим: читаем "сырые" зависимости из файла и строим транзитивный граф
        let raw_graph = load_test_graph(&config.repository)
            .unwrap_or_else(|e| { eprintln!("Ошибка: {}", e); process::exit(1); });
        build_test_graph(&config.name, &raw_graph, config.max_depth)
    } else {
        // Реальный режим: собираем транзитивный граф через crates.io API
        let client = reqwest::blocking::Client::new();
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        let mut visited: HashSet<String> = HashSet::new();
        // Кэши, чтобы не запрашивать одно и то же несколько раз
        let mut latest_cache: HashMap<String, String> = HashMap::new();
        let mut deps_cache: HashMap<String, Vec<String>> = HashMap::new();

        if let Err(e) = build_real_graph(
            &client,
            &config.name,
            &config.version,
            &mut graph,
            &mut visited,
            config.max_depth,
            &mut latest_cache,
            &mut deps_cache,
        ) {
            eprintln!("Ошибка: {}", e);
            process::exit(1);
        }
        graph
    };

    // 3) Печать ASCII-дерева (учитывает max_depth)
    println!("Граф зависимостей для {} v{}:", config.name, config.version);
    print_ascii_tree(
        &graph,
        &config.name,
        "",
        true,
        &mut HashSet::new(),
        0,
        config.max_depth,
    );
}

/// Загружает тестовый граф из файла формата "A: B C"
fn load_test_graph(path: &str) -> Result<HashMap<String, Vec<String>>, String> {
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("Ошибка чтения тестового графа '{}': {}", path, e))?;
    let mut graph = HashMap::new();

    for (lineno, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() { continue; }
        if let Some((pkg, deps)) = line.split_once(':') {
            let pkg = pkg.trim().to_string();
            let deps: Vec<String> = deps
                .split_whitespace()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            graph.insert(pkg, deps);
        } else {
            return Err(format!("Ошибка формата в строке {}: {}", lineno + 1, line));
        }
    }
    Ok(graph)
}

/// Построение транзитивного графа для тестового режима (итеративный DFS без рекурсии)
/// - start: имя корневого пакета
/// - graph_raw: "сырые" прямые зависимости из файла
/// - max_depth: Option<usize> — ограничение глубины (0-based: root depth = 0)
fn build_test_graph(
    start: &str,
    graph_raw: &HashMap<String, Vec<String>>,
    max_depth: Option<usize>,
) -> HashMap<String, Vec<String>> {
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    let mut visited: HashSet<String> = HashSet::new();
    // стек хранит (node, depth)
    let mut stack: Vec<(String, usize)> = vec![(start.to_string(), 0)];

    while let Some((node, depth)) = stack.pop() {
        if visited.contains(&node) {
            continue;
        }
        visited.insert(node.clone());

        // Берём прямые зависимости из исходного файла (или пустой вектор)
        let deps = graph_raw.get(&node).cloned().unwrap_or_default();
        graph.insert(node.clone(), deps.clone());

        // Если есть ограничение глубины и мы достигли его — не углубляемся дальше
        if let Some(max) = max_depth {
            if depth >= max {
                continue;
            }
        }

        // Добавляем детей в стек с увеличенной глубиной
        for dep in deps {
            stack.push((dep, depth + 1));
        }
    }

    graph
}

/// Получение прямых зависимостей конкретной версии через crates.io API
/// Использует кэш deps_cache по ключу "crate:version"
fn fetch_dependencies_cached(
    client: &reqwest::blocking::Client,
    pkg: &str,
    version: &str,
    deps_cache: &mut HashMap<String, Vec<String>>,
) -> Result<Vec<String>, String> {
    let key = format!("{}:{}", pkg, version);
    if let Some(cached) = deps_cache.get(&key) {
        return Ok(cached.clone());
    }

    let url = format!("https://crates.io/api/v1/crates/{}/{}/dependencies", pkg, version);
    let resp = client
        .get(&url)
        .header("User-Agent", "dep-visualizer (edu)")
        .send()
        .map_err(|e| format!("Ошибка HTTP при запросе зависимостей {} {}: {}", pkg, version, e))?;

    if !resp.status().is_success() {
        return Err(format!("crates.io вернул статус {} для {}/{}", resp.status(), pkg, version));
    }

    let deps_resp: DependenciesResponse =
        resp.json().map_err(|e| format!("Ошибка парсинга JSON зависимостей {} {}: {}", pkg, version, e))?;

    let dep_names: Vec<String> = deps_resp
        .dependencies
        .into_iter()
        .filter(|dep| dep.kind.as_deref() != Some("dev"))
        .map(|d| d.crate_id)
        .collect();

    deps_cache.insert(key, dep_names.clone());
    Ok(dep_names)
}

/// Получение последней версии пакета (кэшируется)
fn fetch_latest_version_cached(
    client: &reqwest::blocking::Client,
    pkg: &str,
    latest_cache: &mut HashMap<String, String>,
) -> Result<String, String> {
    if let Some(v) = latest_cache.get(pkg) {
        return Ok(v.clone());
    }

    let url = format!("https://crates.io/api/v1/crates/{}/versions", pkg);
    let resp = client
        .get(&url)
        .header("User-Agent", "dep-visualizer (edu)")
        .send()
        .map_err(|e| format!("Ошибка HTTP при запросе версий {}: {}", pkg, e))?;

    if !resp.status().is_success() {
        return Err(format!("crates.io вернул статус {} при запросе версий {}", resp.status(), pkg));
    }

    let versions: VersionsResponse =
        resp.json().map_err(|e| format!("Ошибка парсинга JSON версий {}: {}", pkg, e))?;
    if let Some(vinfo) = versions.versions.first() {
        latest_cache.insert(pkg.to_string(), vinfo.num.clone());
        Ok(vinfo.num.clone())
    } else {
        Err(format!("Не найдены версии для пакета {}", pkg))
    }
}

/// Построение транзитивного графа для реального пакета через crates.io API
/// Итеративный DFS без рекурсии, с кэшами и ограничением глубины.
/// - client: reqwest client
/// - pkg, version: стартовая вершина и её версия
/// - graph: выходной граф (node -> прямые зависимости)
/// - visited: множество уже обработанных узлов
/// - max_depth: Option<usize> — ограничение глубины (root depth = 0)
/// - latest_cache, deps_cache: кэши для уменьшения числа HTTP-запросов
fn build_real_graph(
    client: &reqwest::blocking::Client,
    pkg: &str,
    version: &str,
    graph: &mut HashMap<String, Vec<String>>,
    visited: &mut HashSet<String>,
    max_depth: Option<usize>,
    latest_cache: &mut HashMap<String, String>,
    deps_cache: &mut HashMap<String, Vec<String>>,
) -> Result<(), String> {
    // стек хранит (node, version, depth)
    let mut stack: Vec<(String, String, usize)> = vec![(pkg.to_string(), version.to_string(), 0)];

    while let Some((node, ver, depth)) = stack.pop() {
        if visited.contains(&node) {
            continue;
        }
        visited.insert(node.clone());

        // Получаем прямые зависимости для node@ver (с кэшем)
        let deps = fetch_dependencies_cached(client, &node, &ver, deps_cache)?;
        graph.insert(node.clone(), deps.clone());

        // Если достигли max_depth — не углубляемся дальше
        if let Some(max) = max_depth {
            if depth >= max {
                continue;
            }
        }

        // Для каждой зависимости получаем её последнюю версию и добавляем в стек
        for dep in deps {
            // Получаем последнюю версию (кэш)
            match fetch_latest_version_cached(client, &dep, latest_cache) {
                Ok(latest_ver) => {
                    stack.push((dep, latest_ver, depth + 1));
                }
                Err(e) => {
                    // Если не удалось получить версию — логируем в stderr и пропускаем
                    eprintln!("Предупреждение: не удалось получить версию для '{}': {}", dep, e);
                }
            }
        }
    }

    Ok(())
}

/// Печать графа в виде ASCII-дерева.
/// - seen предотвращает бесконечные циклы при печати
/// - current_depth и max_depth контролируют глубину печати
fn print_ascii_tree(
    graph: &HashMap<String, Vec<String>>,
    node: &str,
    prefix: &str,
    last: bool,
    seen: &mut HashSet<String>,
    current_depth: usize,
    max_depth: Option<usize>,
) {
    let connector = if last { "└── " } else { "├── " };
    println!("{}{}{}", prefix, connector, node);

    // Если узел уже встречался — помечаем цикл и не углубляемся
    if !seen.insert(node.to_string()) {
        println!("{}    (цикл: узел {})", prefix, node.to_string());
        return;
    }

    // Проверяем ограничение глубины для печати
    if let Some(max) = max_depth {
        if current_depth >= max {
            // показываем, что дальше есть дети, но не раскрываем их
            if let Some(children) = graph.get(node) {
                if !children.is_empty() {
                    println!("{}    ... (ограничение глубины)", prefix);
                }
            }
            return;
        }
    }

    if let Some(children) = graph.get(node) {
        let new_prefix = if last { format!("{}    ", prefix) } else { format!("{}│   ", prefix) };
        for (i, child) in children.iter().enumerate() {
            let is_last = i == children.len() - 1;
            print_ascii_tree(graph, child, &new_prefix, is_last, seen, current_depth + 1, max_depth);
        }
    }
}
