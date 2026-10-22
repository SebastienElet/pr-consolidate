mod forge;
use std::io::{Read, Write};

fn run(args: &[String]) -> i32 {
    match args[0].as_str() {
        "wait" => {
            std::fs::write(&args[1], "ready").unwrap();
            std::io::stdin().read_to_string(&mut String::new()).unwrap();
        }
        "write" => std::fs::write(&args[1], &args[2]).unwrap(),
        "stdin" => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input).unwrap();
            print!("{input}");
            return if input == args[1] { 0 } else { 17 };
        }
        "print" => println!("{}", args[1]),
        "exit" => {
            println!("{}", args[2]);
            return args[1].parse().unwrap();
        }
        "secret" | "secret-sleep" => {
            let token = std::env::var("NPM_TOKEN").unwrap_or_default();
            println!("credential-present={}", !token.is_empty());
            println!("{token}");
            std::io::stdout().flush().unwrap();
            if args[0] == "secret-sleep" {
                std::thread::sleep(std::time::Duration::from_secs(10));
            }
        }
        "spawn" => {
            #[allow(
                clippy::zombie_processes,
                reason = "The fixture must exit before its output-holding child."
            )]
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["child", &args[1], &args[2]])
                .spawn()
                .unwrap();
            if args[1] == "escaped" {
                std::fs::write(&args[2], child.id().to_string()).unwrap();
                std::thread::sleep(std::time::Duration::from_secs(20));
            }
        }
        "child" => match args[1].as_str() {
            "escaped" => {
                assert!(unsafe { libc::setsid() } >= 0);
                std::thread::sleep(std::time::Duration::from_secs(8));
            }
            "late" => {
                std::thread::sleep(std::time::Duration::from_secs(2));
                std::fs::write(&args[2], "still ran").unwrap();
            }
            "short" => {
                std::thread::sleep(std::time::Duration::from_millis(300));
                println!("completed child");
            }
            _ => return 23,
        },
        _ => return 23,
    }
    0
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let name = std::env::args().next().unwrap();
    let code = if std::path::Path::new(&name).file_name().unwrap() == "cargo" {
        std::env::var("FIXTURE_CARGO_STATUS")
            .unwrap()
            .parse()
            .unwrap()
    } else if matches!(
        std::path::Path::new(&name)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap(),
        "gh" | "bkt"
    ) {
        forge::run(&args)
    } else {
        run(&args)
    };
    std::io::stdout().flush().unwrap();
    std::process::exit(code);
}
