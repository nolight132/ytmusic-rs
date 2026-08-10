use serde_json::Value;

pub trait Nav {
    fn at(&self, path: &[&str]) -> Option<&Value>;
    fn str_at(&self, path: &[&str]) -> Option<&str>;
    fn items(&self, path: &[&str]) -> &[Value];
    fn run_text(&self, path: &[&str]) -> Option<String>;
    fn runs(&self, path: &[&str]) -> &[Value];
}

impl Nav for Value {
    fn at(&self, path: &[&str]) -> Option<&Value> {
        let mut node = self;
        for key in path {
            node = match key.parse::<usize>() {
                Ok(index) => node.get(index)?,
                Err(_) => node.get(key)?,
            };
        }
        Some(node)
    }

    fn str_at(&self, path: &[&str]) -> Option<&str> {
        self.at(path)?.as_str()
    }

    fn items(&self, path: &[&str]) -> &[Value] {
        self.at(path)
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }

    fn run_text(&self, path: &[&str]) -> Option<String> {
        let node = self.at(path)?;
        if let Some(text) = node.as_str() {
            return Some(text.to_string());
        }
        if let Some(text) = node.get("simpleText").and_then(Value::as_str) {
            return Some(text.to_string());
        }
        let runs = node.get("runs")?.as_array()?;
        let text: String = runs
            .iter()
            .filter_map(|run| run.get("text").and_then(Value::as_str))
            .collect();
        match text.is_empty() {
            true => None,
            false => Some(text),
        }
    }

    fn runs(&self, path: &[&str]) -> &[Value] {
        self.at(path)
            .and_then(|node| node.get("runs"))
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }
}

pub fn find_all<'a>(node: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match node {
        Value::Object(map) => {
            for (k, v) in map {
                match k == key {
                    true => out.push(v),
                    false => find_all(v, key, out),
                }
            }
        }
        Value::Array(list) => {
            for v in list {
                find_all(v, key, out);
            }
        }
        _ => {}
    }
}
