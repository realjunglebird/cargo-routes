use std::env;                               // для парсинга параметров запуска
use std::fs::{self, File};                  // для работы с файловой системой
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use serde::Deserialize;                     // для макроса автодесериализации
use serde_json;                             // для парсинга JSON

// Структура файла конфигурации
#[derive(Deserialize, Debug)]
struct Config {
    package_name: String,       // имя анализируемого пакета
    repository: String,         // URL-адрес репозитория или путь к файлу
    test_repo_mode: String,     // режим работы с тестовым репозиторием
    version: String,            // версия пакета
    output_filename: String,    // имя сгенерированного файла с изображением графа
    ascii_tree_mode: bool,      // режим вывода зависимостей в формате ASCII-дерева
}

fn main() {
    // 1 - Получаем параметры запуска
    let args: Vec<String> = env::args().collect();

    // Проверяем что первым аргументом передан путь к файлу конфигурации
    if args.len() < 2 {
        eprintln!("Ошибка: необходимо указать путь к JSON-конфигу.\n\
                   Пример: cargo run -- config.json");
        process::exit(1);
    }

    // Путь к конфигу
    let config_path = &args[1];

    // 2 - Помещаем JSON файл в строку. В случае ошибки выводим её текст и выходим.
    let raw = match fs::read_to_string(config_path) {
        Ok(content) => content,
        Err(err) => {
            eprintln!("Не удалось прочитать файл '{}' : {}", config_path, err);
            process::exit(1);
        }
    };

    // 3 - Парсим JSON в структуру Config. В случае ошибки выводим её текст и выходим.
    let config: Config = match serde_json::from_str(&raw) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Ошибка парсинга JSON в '{}' : {}", config_path, err);
            process::exit(1);
        }
    };

    // 4 - Проверяем все параметры на корректность
    if config.package_name.trim().is_empty() {
        eprintln!("Ошибка: поле package_name не может быть пустым!");
        process::exit(1);
    }
    if config.repository.trim().is_empty() {
        eprintln!("Ошибка: поле repository не может быть пустым!");
        process::exit(1);
    }
    if config.test_repo_mode.trim().is_empty() {
        eprintln!("Ошибка: поле test_repo_mode не может быть пустым!");
        process::exit(1);
    }
    if config.version.trim().is_empty() {
        eprintln!("Ошибка: поле version не может быть пустым!");
        process::exit(1);
    }
    if config.output_filename.trim().is_empty() {
        eprintln!("Ошибка: поле output_filename не может быть пустым!");
        process::exit(1);
    }

    // Получаем путь к репозиторию
    let repo_dir = match fetch_repository(&config) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Ошибка загрузки репозитория: {}", e);
            process::exit(1);
        }
    };

    // Извлекаем и выводим прямые зависимости
    match list_direct_dependencies(&repo_dir) {
        Ok(deps) => {
            println!("Прямые зависимости для пакета '{}' версии '{}':",
                     config.package_name.trim(),
                     config.version.trim(),);
            for dep in deps {
                println!("- {}", dep);
            }
        }
        Err(e) => {
            eprintln!("Ошибка при анализе зависимостей: {}", e);
            process::exit(1);
        }
    }

    // Удаляем временную директорию, если она была создана
    if config.test_repo_mode.trim() == "remote" {
        if let Err(e) = fs::remove_dir_all(&repo_dir) {
            eprintln!("Не удалось удалить временную директорию '{}' : {}", repo_dir.display(), e);
        }
    }
}

/// Функция для загрузки репозитория:
/// - если test_repo_mode - local, то просто возвращает путь
/// - если remote, то клонирует нужную версию во временную директорию и возвращает путь
fn fetch_repository(config: &Config) -> Result<PathBuf, String> {
    if config.test_repo_mode == "local" {
        let p = PathBuf::from(config.repository.trim());
        if p.join("Cargo.toml").exists() {
            return Ok(p);
        } else {
            return Err(format!("В локальной директории {} не найден Cargo.toml", p.display()));
        }
    }

    // Создаём уникальную временную директорию
    let tmp_base = env::temp_dir();
    let repo_dir = tmp_base.join(format!(
        "{}-{}-{}",
        config.package_name,
        config.version,
        process::id()
    ));

    // Удаляем если она уже существует
    let _ = fs::remove_dir_all(&repo_dir);

    // Клонируем нужную версию с помощью git
    let status = Command::new("git")
        .args(&[
            "clone",
            "--branch",
            &config.version,
            "--depth",
            "1",
            &config.repository,
            repo_dir.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .map_err(|e| format!("Не удалось запустить git: {}", e))?;

    if !status.success() {
        return Err(format!("Ошибка git clone: статус {}", status));
    }

    Ok(repo_dir)
}

/// Извлекает прямые зависимости из секции [dependencies] в Cargo.toml
/// Возвращает имена зависимостей в виде списка
fn list_direct_dependencies(repo_dir: &Path) -> io::Result<Vec<String>> {
    let toml_path = repo_dir.join("Cargo.toml");
    let file = File::open(&toml_path)?;
    let reader = io::BufReader::new(file);

    let mut deps = Vec::new();
    let mut in_deps_section = false;

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();

        // Начало секции с зависимостями
        if trimmed == "[dependencies]" {
            in_deps_section = true;
            continue;
        }

        // Выход из секции при встрече новой
        if in_deps_section && trimmed.starts_with("[") {
            break;
        }

        // Парсим строки вида [имя = версия]
        if in_deps_section && !trimmed.is_empty() && !trimmed.starts_with("#") {
            if let Some((key, _)) = trimmed.split_once('=') {
                deps.push(key.trim().trim_matches('"').to_string());
            }
        }
    }

    Ok(deps)
}
