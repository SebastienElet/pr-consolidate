use serde_json::{Value, json};
use std::io::Read;

fn persist(path: &std::path::Path, state: &Value) {
    std::fs::write(path, serde_json::to_vec(state).unwrap()).unwrap();
}
pub fn run(args: &[String]) -> i32 {
    let path = std::path::PathBuf::from(std::env::var_os("FORGE_FIXTURE_STATE").unwrap());
    let mut state: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(args[0], "api");
    let route = args[1].split('?').next().unwrap();
    let method = &args[args.iter().position(|a| a == "--method").unwrap() + 1];
    let payload = args.iter().position(|a| a == "--input").map(|index| {
        if args[index + 1] != "-" {
            return serde_json::from_str::<Value>(&args[index + 1]).unwrap();
        }
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).unwrap();
        serde_json::from_str::<Value>(&input).unwrap()
    });
    state["argv"].as_array_mut().unwrap().push(json!(args));
    state["calls"]
        .as_array_mut()
        .unwrap()
        .push(json!([method, args[1], payload]));
    if state["invalid_output"] == true {
        persist(&path, &state);
        println!("private-cli-output");
        return 0;
    }
    if let Some(code) = state["exit_code"].as_i64() {
        persist(&path, &state);
        eprintln!("private-cli-output");
        return code as i32;
    }
    let github = state["forge"] == "github";
    let account = state["resources"]["/user"].clone();
    let resources = state["resources"].as_object_mut().unwrap();
    let result = if method == "GET" {
        let resource = resources
            .get(route)
            .unwrap_or_else(|| panic!("unexpected GET {route}"));
        if let Some(values) = resource.as_array() {
            let page = args[1]
                .split('?')
                .nth(1)
                .unwrap_or("")
                .split('&')
                .find_map(|v| v.strip_prefix("page="))
                .unwrap_or("1")
                .parse::<usize>()
                .unwrap();
            let values: Vec<_> = values
                .iter()
                .skip((page - 1) * 100)
                .take(100)
                .cloned()
                .collect();
            if github {
                json!(values)
            } else {
                json!({"size":values.len(),"values":values})
            }
        } else {
            resource.clone()
        }
    } else if method == "POST" && (route.ends_with("/pulls") || route.ends_with("/pullrequests")) {
        let mut created = payload.unwrap();
        if github {
            created = json!({"id":1901,"number":901,"title":created["title"],"body":created["body"],"user":{"id":9,"login":"author"},"head":{"sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","ref":created["head"]},"base":{"ref":created["base"]},"state":"open","draft":false,"merged":false,"merged_at":null,"merge_commit_sha":null,"changed_files":1});
        } else {
            created["id"] = json!(901);
            created["state"] = json!("OPEN");
            created["source"]["commit"] =
                json!({"hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"});
        }
        resources
            .get_mut(route)
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(created.clone());
        resources.insert(format!("{route}/901"), created.clone());
        created
    } else if let Some(resource) = resources.get_mut(route) {
        if let Some(collection) = resource.as_array_mut() {
            let mut item = payload.unwrap();
            item["id"] = json!(
                collection
                    .iter()
                    .filter_map(|v| v["id"].as_u64())
                    .max()
                    .unwrap_or(0)
                    + 1
            );
            item["user"] = account;
            collection.push(item.clone());
            item
        } else {
            for (key, value) in payload.unwrap().as_object().unwrap() {
                resource[key] = value.clone();
            }
            resource.clone()
        }
    } else {
        let (parent, id) = route.rsplit_once('/').unwrap();
        let id: u64 = id.parse().unwrap();
        let collection = if route.contains("/issues/comments/") {
            resources
                .iter_mut()
                .find(|(key, value)| {
                    key.ends_with("/comments")
                        && value
                            .as_array()
                            .is_some_and(|v| v.iter().any(|item| item["id"] == id))
                })
                .unwrap()
                .1
        } else {
            resources
                .get_mut(parent)
                .unwrap_or_else(|| panic!("unexpected {method} {route}"))
        };
        let item = collection
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|v| v["id"] == id)
            .unwrap();
        for (key, value) in payload.unwrap().as_object().unwrap() {
            item[key] = value.clone();
        }
        item.clone()
    };
    let fail = method != "GET" && state["accepted_write_failure"] == true;
    if fail {
        state
            .as_object_mut()
            .unwrap()
            .remove("accepted_write_failure");
    }
    persist(&path, &state);
    if fail {
        eprintln!("private-cli-output");
        return 23;
    }
    println!("{result}");
    0
}
