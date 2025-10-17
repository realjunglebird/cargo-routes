use std::env;               // для парсинга параметров запуска
use std::fs;                // для работы с файловой системой
use std::process;           // для завершения программы с кодом ошибки
use serde::Deserialize;     // для макроса автодесериализации
use serde_json;             // для парсинга JSON

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
    // Получаем параметры запуска
    let args: Vec<String> = env::args().collect();

    // Проверяем что первым аргументом передан путь к файлу конфигурации
    if args.len() < 2 {
        eprintln!("Ошибка: необходимо указать путь к JSON-конфигу.\n\
                   Пример: cargo run -- config.json");
        process::exit(1);
    }

    // Путь к конфигу
    let config_path = &args[1];

    // Помещаем JSON файл в строку. В случае ошибки выводим её текст и выходим.
    let raw = match fs::read_to_string(config_path) {
        Ok(content) => content,
        Err(err) => {
            eprintln!("Не удалось прочитать файл '{}' : {}", config_path, err);
            process::exit(1);
        }
    };

    // Парсим JSON в структуру Config. В случае ошибки выводим её текст и выходим.
    let config: Config = match serde_json::from_str(&raw) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Ошибка парсинга JSON в '{}' : {}", config_path, err);
            process::exit(1);
        }
    };

    // Выводим все параметры конфигурации в формате ключ : значение
    println!("package_name: {}", config.package_name);
    println!("repository: {}", config.repository);
    println!("test_repo_mode: {}", config.test_repo_mode);
    println!("version: {}", config.version);
    println!("output_filename: {}", config.output_filename);
    println!("ascii_tree_mode: {}", config.ascii_tree_mode);
}
