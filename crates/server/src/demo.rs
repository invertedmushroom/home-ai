use crate::{
    artifacts::{File, FileChange},
    model::{Completion, Mode},
};
use serde_json::json;
use std::time::Duration;

pub(crate) async fn complete(prompt: &str, mode: Mode, base: &[File]) -> Completion {
    tokio::time::sleep(Duration::from_millis(500)).await;
    let text = if mode == Mode::Files {
        let step = if base
            .iter()
            .any(|file| file.path == "app.js" && file.content.contains("const step = 1;"))
        {
            2
        } else {
            1
        };
        let desired = if prompt.contains("/output") {
            vec![File { path: "build.cjs".into(), content: r#"const fs = require('node:fs');
const html = `<!doctype html><html lang="en"><meta charset="utf-8"><title>Worker output demo</title><style>body{font:18px system-ui;background:#eef3ef;color:#183b2c;text-align:center;padding:30px}strong{display:block;font-size:64px;margin:20px}button{padding:12px 24px}</style><h1>Built in the Node worker</h1><p>This file reached the project through a reviewed execution proposal.</p><strong id="count">0</strong><button id="add">Add one</button><script>let count=0;document.querySelector('#add').onclick=()=>{document.querySelector('#count').textContent=String(++count);};</script></html>`;
fs.writeFileSync('/output/index.html', html);
console.log('Created index.html in /output. Review its proposal before applying.');
"#.into() }]
        } else {
            vec![
            File { path: "index.html".into(), content: "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Project counter</title><link rel=\"stylesheet\" href=\"style.css\"></head><body><h1>Project counter</h1><strong id=\"count\">0</strong><button id=\"add\">Add one</button><script src=\"app.js\"></script></body></html>".into() },
            File { path: "style.css".into(), content: "body{font:18px system-ui;background:#eef3ef;color:#183b2c;text-align:center}strong{display:block;font-size:64px;margin:20px}button{padding:12px 24px}".into() },
            File { path: "app.js".into(), content: format!("const step = {step};\nlet count = 0;\ndocument.querySelector('#add').onclick = () => {{ document.querySelector('#count').textContent = String(count += step); }};\n") },
        ]
        };
        let changes: Vec<_> = desired
            .into_iter()
            .filter(|file| !base.contains(file))
            .map(|file| FileChange::Write {
                path: file.path,
                content: file.content,
            })
            .collect();
        format!(
            "This scripted demo proposes files; review before applying.\n\n```files\n{}\n```",
            json!({"changes":changes})
        )
    } else if mode == Mode::App {
        "Here is a small counter app. This demo is scripted; your request is saved but does not change the template.\n\n```html\n<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Little counter</title><style>body{font:18px system-ui;background:#eef3ef;color:#183b2c;display:grid;place-content:center;min-height:90vh;text-align:center}button{font:inherit;padding:12px 24px;border:0;border-radius:30px;background:#246044;color:white}strong{font-size:64px;display:block;margin:20px}</style><h1>A little room to experiment</h1><p>Every response can become a separate revision.</p><strong id=\"count\">0</strong><button onclick=\"document.querySelector('#count').textContent=++window.count\">Add one</button><script>window.count=0</script></html>\n```".into()
    } else {
        format!(
            "Demo mode is working. I received:\n\n{prompt}\n\nChoose “Change project files” to create a reviewed proposal, or select a configured model backend. This response is scripted, not model inference."
        )
    };
    Completion {
        text,
        ..Default::default()
    }
}
