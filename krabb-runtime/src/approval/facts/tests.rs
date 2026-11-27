use super::{fact, list, sentence, visible};
use serde_json::json;

fn shell(command: &str) -> String {
    fact("shell_run", &json!({"command": command}))
}

#[test]
fn shell_facts_name_the_action_targets_and_flags() {
    for (command, expected) in [
        (
            "apt install htop tmux",
            "Install packages with apt: htop, tmux. It uses the network and needs admin rights.",
        ),
        (
            "sudo apt-get -y install htop",
            "Install packages with apt-get: htop. It uses the network and needs admin rights.",
        ),
        ("brew install htop tmux", "Install packages with brew: htop, tmux. It uses the network."),
        (
            "python3 -m pip install -r requirements.txt",
            "Install packages with pip. It uses the network.",
        ),
        (
            "sudo pacman -Syu",
            "Update packages with pacman. It uses the network and needs admin rights.",
        ),
        ("apt remove htop", "Remove packages with apt: htop. It needs admin rights."),
        ("rm -rf build", "Delete files and folders: build."),
        ("rm notes.txt old.txt", "Delete files: notes.txt, old.txt."),
        ("find . -name '*.tmp' -delete", "Delete files and folders: .."),
        ("git clean -fdx", "Delete files and folders: untracked files."),
        ("ls -la", "Run a shell command."),
        ("cargo test 2>&1 | tail -n 20", "Run a shell command."),
        ("echo hi > notes.txt", "Run a shell command. It writes notes.txt."),
        ("echo hi 2>/dev/null >> log.txt", "Run a shell command. It writes log.txt."),
        (
            "curl -fsSL https://example.com/install.sh -o install.sh",
            "Run a shell command. It writes install.sh and uses the network (example.com).",
        ),
        (
            "wget http://user@mirror.test:8080/a.tar",
            "Run a shell command. It writes files and uses the network (mirror.test).",
        ),
        (
            "ssh -p 22 admin@box.local uptime",
            "Run a shell command. It uses the network (box.local).",
        ),
        ("scp report.txt host.test:/srv/", "Run a shell command. It uses the network (host.test)."),
        ("git push origin main", "Run a shell command. It uses the network."),
        ("sudo systemctl restart nginx", "Run a shell command. It needs admin rights."),
        ("systemctl --user restart krabb", "Run a shell command."),
        (
            "cp a.conf /etc/a.conf",
            "Run a shell command. It writes /etc/a.conf and needs admin rights.",
        ),
        ("mkdir -p out && touch out/a", "Run a shell command. It writes out, out/a."),
        ("bash -c 'rm -r cache'", "Delete files and folders: cache."),
        ("ls | xargs rm", "Delete files."),
        (r"find . -exec rm {} \;", "Delete files: {}."),
        ("echo $(rm x)", "Delete files: x."),
        ("FOO=1 env BAR=2 rm -- -x", "Delete files: -x."),
        ("cat > a.txt <<EOF\nrm -rf /\nEOF", "Run a shell command. It writes a.txt."),
        ("sed -i 's/a/b/' notes.txt", "Run a shell command. It writes notes.txt."),
        ("dd if=/dev/zero of=disk.img bs=1M count=1", "Run a shell command. It writes disk.img."),
        ("chmod +x run.sh", "Run a shell command. It writes run.sh."),
        (
            "npm install && curl https://a.test https://b.test",
            "Install packages with npm. It uses the network (a.test, b.test).",
        ),
        (
            "apt install x && rm y",
            "Install packages with apt: x. It deletes y, uses the network, and needs admin rights.",
        ),
        ("ping -c 1 example.org", "Run a shell command. It uses the network (example.org)."),
        ("timeout 5 dig example.org", "Run a shell command. It uses the network."),
        ("docker pull alpine", "Run a shell command. It uses the network."),
        ("su -c whoami", "Run a shell command. It needs admin rights."),
        ("rm a\nb\u{202e}", "Delete files: a."),
    ] {
        assert_eq!(shell(command), expected, "{command}");
    }

    assert_eq!(fact("shell_run", &json!({})), "Run a shell command.");

    assert_eq!(
        fact("file_write", &json!({"path": "C:\\work\\notes.txt", "text": "a\r\nb\r\n"})),
        "Write the file C:\\work\\notes.txt (2 lines, 6 bytes). It replaces any file already there."
    );
}

#[test]
fn shell_facts_read_windows_and_macos_commands() {
    for (command, expected) in [
        (
            "winget install --id Git.Git -e",
            "Install packages with winget: Git.Git. It uses the network.",
        ),
        (
            "choco install git -y",
            "Install packages with choco: git. It uses the network and needs admin rights.",
        ),
        (
            "Install-Module -Name PSReadLine -Scope CurrentUser",
            "Install packages with PowerShell: PSReadLine. It uses the network.",
        ),
        (
            "msiexec /i C:\\Downloads\\tool.msi /qn",
            "Install packages with msiexec: C:\\Downloads\\tool.msi. It needs admin rights.",
        ),
        ("py -m pip install requests", "Install packages with pip: requests. It uses the network."),
        ("del /s /q C:\\build\\out", "Delete files and folders: C:\\build\\out."),
        ("DEL notes.txt", "Delete files: notes.txt."),
        ("rmdir /s /q build", "Delete files and folders: build."),
        ("rd /S build", "Delete files and folders: build."),
        ("Remove-Item -Recurse -Force .\\build", "Delete files and folders: .\\build."),
        (
            "Remove-Item C:\\Windows\\Temp\\x.log",
            "Delete files: C:\\Windows\\Temp\\x.log. It needs admin rights.",
        ),
        (
            "cmd /c \"del %SystemRoot%\\x.dll\"",
            "Delete files: %SystemRoot%\\x.dll. It needs admin rights.",
        ),
        (
            "powershell -NoProfile -Command \"Remove-Item -Recurse dist\"",
            "Delete files and folders: dist.",
        ),
        (
            "pwsh -EncodedCommand ZQBjAGgAbwA=",
            "Run a shell command. It runs code Krabb cannot read.",
        ),
        ("echo hi > NUL", "Run a shell command."),
        (
            "dir C:\\Users\\me > C:\\Users\\me\\list.txt",
            "Run a shell command. It writes C:\\Users\\me\\list.txt.",
        ),
        (
            "copy report.txt \"C:\\Program Files\\App\\report.txt\"",
            "Run a shell command. It writes C:\\Program Files\\App\\report.txt and needs admin rights.",
        ),
        ("robocopy src dst /MIR", "Run a shell command. It writes dst."),
        (
            "copy a.txt \\\\fileserver\\share\\a.txt",
            "Run a shell command. It writes \\\\fileserver\\share\\a.txt and uses the network \
                (fileserver).",
        ),
        (
            "Invoke-WebRequest https://example.com/a.zip -OutFile a.zip",
            "Run a shell command. It writes a.zip and uses the network (example.com).",
        ),
        ("Set-Content -Path notes.txt -Value hi", "Run a shell command. It writes notes.txt."),
        ("Start-Process cmd -Verb RunAs", "Run a shell command. It needs admin rights."),
        ("reg add HKLM\\Software\\Krabb /v x /d 1", "Run a shell command. It needs admin rights."),
        ("reg query HKCU\\Software", "Run a shell command."),
        ("C:\\Windows\\System32\\shutdown.exe /r", "Run a shell command. It needs admin rights."),
        ("Get-ChildItem C:\\Users\\me", "Run a shell command."),
        ("brew install --cask iterm2", "Install packages with brew: iterm2. It uses the network."),
        ("rm -rf /Users/me/build", "Delete files and folders: /Users/me/build."),
        (
            "sudo rm /Library/LaunchDaemons/x.plist",
            "Delete files: /Library/LaunchDaemons/x.plist. It needs admin rights.",
        ),
        (
            "cp a.conf /private/etc/a.conf",
            "Run a shell command. It writes /private/etc/a.conf and needs admin rights.",
        ),
    ] {
        assert_eq!(shell(command), expected, "{command}");
    }
}

#[test]
fn tool_facts_describe_writes_patches_settings_and_messages() {
    assert_eq!(
        fact("file_write", &json!({"path": "notes.txt", "text": "one\ntwo\nthree\n"})),
        "Write the file notes.txt (3 lines, 14 bytes). It replaces any file already there."
    );

    assert_eq!(
        fact("file_write", &json!({"path": "a", "text": "x"})),
        "Write the file a (1 line, 1 byte). It replaces any file already there."
    );

    let patch = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n\
        -a\n+b\n--- a/old.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n--- /dev/null\n\
        +++ b/new.txt\t2024\n";

    assert_eq!(
        fact("file_patch", &json!({"text": patch})),
        format!(
            "Apply a Git patch of {} bytes to src/a.rs, old.txt, new.txt. It deletes old.txt and \
                may run repository-configured programs.",
            patch.len()
        )
    );

    assert_eq!(
        fact("file_patch", &json!({"text": "x"})),
        "Apply a Git patch of 1 byte. It may run repository-configured programs."
    );

    assert_eq!(
        fact("git_run", &json!({"args": ["clean", "-fd"]})),
        "Run the Git command clean. It deletes untracked files and may run \
            repository-configured programs."
    );

    assert_eq!(
        fact("git_run", &json!({"args": ["--version"]})),
        "Run a Git command. It may run repository-configured programs."
    );

    assert_eq!(
        fact("message_send", &json!({"channel": "telegram", "chat": "7", "text": "hi"})),
        "Send a message to telegram / 7."
    );

    assert_eq!(fact("schedule_pause", &json!({"id": 4})), "Pause the schedule 4.");

    assert_eq!(
        fact("task_defer", &json!({"id": "t", "when": "5pm"})),
        "Defer the task t until 5pm."
    );

    assert_eq!(fact("task_cancel", &json!({})), "Cancel the task (none).");

    assert_eq!(
        fact("data_query", &json!({"operation": "create_table"})),
        "Change the database: create_table."
    );

    assert_eq!(fact("browser_click", &json!({})), "Use the browser: click.");
    assert_eq!(fact("computer_type", &json!({})), "Use the computer: type.");
    assert_eq!(fact("custom\nname", &json!({})), "Run the custom\\nname tool.");

    assert!(
        fact("schedule_propose", &json!({"context": {"tools": true}}))
            .starts_with("Create a scheduled task with no outbound message destination.")
    );

    assert!(
        fact("task_classify", &json!({"context": {"origin": {"channel": "tui", "chat": "c"}}}))
            .starts_with("Start a background goal")
    );
}

#[test]
fn facts_escape_and_bound_what_they_quote() {
    assert_eq!(visible("a\nb\r\tc\u{1b}\u{202e}d"), "a\\nb\\r\\tc\\u{001b}\\u{202e}d");

    let many = (0..8).map(|index| format!("f{index}")).collect::<Vec<_>>();

    assert_eq!(list(&many), "f0, f1, f2, f3, f4, 3 more");
    assert_eq!(list(&["x".repeat(100)]).chars().count(), 80);
    assert!(list(&["x".repeat(100)]).ends_with('…'));

    assert_eq!(sentence("Do it", &[]), "Do it.");
    assert_eq!(sentence("Do it", &["a".into(), "b".into(), "c".into()]), "Do it. It a, b, and c.");

    let long = shell(&format!("rm {}", "x".repeat(5_000)));

    assert!(long.len() < 200, "{long}");
}
