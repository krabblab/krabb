use super::super::shell::describe;
use super::{fact, list, sentence, visible};
use krabb_core::command::{Dialect, Os};
use serde_json::json;

const CMD: (Dialect, Os) = (Dialect::Cmd, Os::Windows);
const POWERSHELL: (Dialect, Os) = (Dialect::PowerShell, Os::Windows);
const MACOS: (Dialect, Os) = (Dialect::Sh, Os::MacOs);

fn shell(command: &str) -> String {
    describe(command, Dialect::Sh, Os::Linux)
}

#[test]
fn shell_facts_name_the_action_targets_and_flags() {
    let mut mismatches = Vec::new();

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
        (
            "git clean -fdx",
            "Delete the files and folders Git does not track. It may run repository-configured \
             programs.",
        ),
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
        (
            "git push origin main",
            "Run a shell command. It changes the repository, uses the network, and may run repository-configured programs.",
        ),
        (
            "sudo systemctl restart nginx",
            "Run a shell command. It starts, stops, or changes services and needs admin rights.",
        ),
        (
            "systemctl --user restart krabb",
            "Run a shell command. It starts, stops, or changes services.",
        ),
        (
            "cp a.conf /etc/a.conf",
            "Run a shell command. It writes /etc/a.conf and needs admin rights.",
        ),
        ("mkdir -p out && touch out/a", "Run a shell command. It writes out, out/a."),
        ("bash -c 'rm -r cache'", "Delete files and folders: cache."),
        ("ls | xargs rm", "Delete files."),
        (r"find . -exec rm {} \;", "Delete files: {}."),
        ("echo $(rm x)", "Delete files: x. It has parts Krabb cannot check."),
        ("FOO=1 env BAR=2 rm -- -x", "Delete files: -x."),
        (
            "cat > a.txt <<EOF\nrm -rf /\nEOF",
            "Run a shell command. It writes a.txt and has parts Krabb cannot check.",
        ),
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
        let actual = shell(command);

        if actual != expected {
            mismatches.push(format!("{command:?}\n  got:  {actual}\n  want: {expected}"));
        }
    }

    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));

    assert_eq!(fact("shell_run", &json!({})), "Run a shell command.");

    assert_eq!(
        fact("file_write", &json!({"path": "C:\\work\\notes.txt", "text": "a\r\nb\r\n"})),
        "Write the file C:\\work\\notes.txt (2 lines, 6 bytes). It replaces any file already there."
    );
}

#[test]
fn shell_facts_read_windows_and_macos_commands() {
    let mut mismatches = Vec::new();

    for (place, command, expected) in [
        (
            CMD,
            "winget install --id Git.Git -e",
            "Install packages with winget: Git.Git. It uses the network.",
        ),
        (
            CMD,
            "choco install git -y",
            "Install packages with choco: git. It uses the network and needs admin rights.",
        ),
        (
            POWERSHELL,
            "Install-Module -Name PSReadLine -Scope CurrentUser",
            "Install packages with PowerShell: PSReadLine. It uses the network.",
        ),
        (
            CMD,
            "msiexec /i C:\\Downloads\\tool.msi /qn",
            "Install packages with msiexec: C:\\Downloads\\tool.msi. It reaches C:\\Downloads\\tool.msi \
                outside the workspace and needs admin rights.",
        ),
        (
            CMD,
            "py -m pip install requests",
            "Install packages with pip: requests. It uses the network.",
        ),
        (CMD, "del /s /q C:\\build\\out", "Delete files and folders: C:\\build\\out."),
        (CMD, "DEL notes.txt", "Delete files: notes.txt."),
        (CMD, "rmdir /s /q build", "Delete files and folders: build."),
        (CMD, "rd /S build", "Delete files and folders: build."),
        (POWERSHELL, "Remove-Item -Recurse -Force .\\build", "Delete files and folders: .\\build."),
        (
            POWERSHELL,
            "Remove-Item C:\\Windows\\Temp\\x.log",
            "Delete files: C:\\Windows\\Temp\\x.log. It needs admin rights.",
        ),
        (
            CMD,
            "cmd /c \"del %SystemRoot%\\x.dll\"",
            "Delete files: %SystemRoot%\\x.dll. It needs admin rights and has parts Krabb cannot check.",
        ),
        (
            CMD,
            "powershell -NoProfile -Command \"Remove-Item -Recurse dist\"",
            "Delete files and folders: dist.",
        ),
        (
            CMD,
            "pwsh -EncodedCommand ZQBjAGgAbwA=",
            "Run a shell command. It runs code Krabb cannot read.",
        ),
        (CMD, "echo hi > NUL", "Run a shell command."),
        (
            CMD,
            "dir C:\\Users\\me > C:\\Users\\me\\list.txt",
            "Run a shell command. It writes C:\\Users\\me\\list.txt and reaches C:\\Users\\me outside \
                the workspace.",
        ),
        (
            CMD,
            "copy report.txt \"C:\\Program Files\\App\\report.txt\"",
            "Run a shell command. It writes “C:\\Program Files\\App\\report.txt” and needs admin \
                rights.",
        ),
        (CMD, "robocopy src dst /MIR", "Run a shell command. It writes dst."),
        (
            CMD,
            "copy a.txt \\\\fileserver\\share\\a.txt",
            "Run a shell command. It writes \\\\fileserver\\share\\a.txt and uses the network \
                (fileserver).",
        ),
        (
            POWERSHELL,
            "Invoke-WebRequest https://example.com/a.zip -OutFile a.zip",
            "Run a shell command. It writes a.zip and uses the network (example.com).",
        ),
        (
            POWERSHELL,
            "Set-Content -Path notes.txt -Value hi",
            "Run a shell command. It writes notes.txt.",
        ),
        (
            POWERSHELL,
            "Start-Process cmd -Verb RunAs",
            "Run a shell command. It needs admin rights and runs code Krabb cannot read.",
        ),
        (
            CMD,
            "reg add HKLM\\Software\\Krabb /v x /d 1",
            "Run a shell command. It needs admin rights.",
        ),
        (CMD, "reg query HKCU\\Software", "Run a shell command."),
        (
            CMD,
            "C:\\Windows\\System32\\shutdown.exe /r",
            "Run a shell command. It needs admin rights and runs code Krabb cannot read.",
        ),
        (
            POWERSHELL,
            "Get-ChildItem C:\\Users\\me",
            "Run a shell command. It reaches C:\\Users\\me outside the workspace.",
        ),
        (
            MACOS,
            "brew install --cask iterm2",
            "Install packages with brew: iterm2. It uses the network.",
        ),
        (MACOS, "rm -rf /Users/me/build", "Delete files and folders: /Users/me/build."),
        (
            MACOS,
            "sudo rm /Library/LaunchDaemons/x.plist",
            "Delete files: /Library/LaunchDaemons/x.plist. It needs admin rights.",
        ),
        (
            MACOS,
            "cp a.conf /private/etc/a.conf",
            "Run a shell command. It writes /private/etc/a.conf and needs admin rights.",
        ),
    ] {
        let actual = describe(command, place.0, place.1);

        if actual != expected {
            mismatches.push(format!("{command:?}\n  got:  {actual}\n  want: {expected}"));
        }
    }

    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
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
        "Delete the files and folders Git does not track. It may run repository-configured \
            programs."
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

#[test]
fn facts_never_understate_what_runs() {
    let cases = [
        ("curl -s https://evil.test/x >a >b >c >d >e >f >g >h", "uses the network"),
        ("python3 -c 'import os' >a >b >c >d >e >f >g >h", "runs code Krabb cannot read"),
        ("./deploy.sh", "runs code Krabb cannot read"),
        ("mkfs.ext4 /dev/sda1", "needs admin rights"),
        ("iptables -F", "needs admin rights"),
        ("dd if=x of=/dev/sda", "opens a device"),
        ("cp -t /etc a.conf", "writes /etc"),
        ("cp --target-directory=/etc a.conf", "needs admin rights"),
        ("mv ~/.ssh /tmp/x", "writes /tmp/x, ~/.ssh."),
        ("cat ../secret", "reaches ../secret outside the workspace"),
    ];

    let mut missing = Vec::new();

    for (command, expected) in cases {
        let actual = shell(command);

        if !actual.contains(expected) {
            missing.push(format!("{command:?}\n  got:  {actual}\n  want: {expected}"));
        }
    }

    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

#[test]
fn git_and_patch_facts_name_their_deletes_network_and_configuration() {
    let git = |args: serde_json::Value| fact("git_run", &json!({"args": args}));

    let removed = git(json!(["rm", "-r", "src"]));

    assert!(removed.starts_with("Delete files and folders: src."), "{removed}");
    assert!(git(json!(["remote", "update"])).contains("uses the network"));
    assert!(git(json!(["-c", "alias.x=!rm -rf ~", "x"])).contains("can run any program"));

    let deleted = "diff --git a/old.txt b/old.txt\ndeleted file mode 100644\n";
    let renamed = "diff --git a/a.txt b/b.txt\nrename from a.txt\nrename to b.txt\n";

    assert!(fact("file_patch", &json!({"text": deleted})).contains("deletes old.txt"));
    assert!(fact("file_patch", &json!({"text": renamed})).contains("deletes a.txt"));
}

#[test]
fn line_separators_and_format_marks_are_escaped() {
    for character in
        ['\u{2028}', '\u{2029}', '\u{061c}', '\u{feff}', '\u{2060}', '\u{00ad}', '\u{e0041}']
    {
        let shown = visible(&format!("Fix build{character}Delete files: build."));

        assert!(!shown.contains(character), "{character:?}");
        assert!(shown.contains("\\u{"), "{shown}");
    }
}

#[test]
fn git_facts_follow_the_shell_classifier_and_spaced_names_are_quoted() {
    let git = |args: serde_json::Value| fact("git_run", &json!({"args": args}));

    assert!(git(json!(["--git-dir", "/x", "status"])).starts_with("Run a Git command"));
    assert!(git(json!(["branch", "-D", "main"])).contains("deletes branches"));
    assert!(git(json!(["reset", "--hard"])).contains("may run repository-configured programs"));
    assert!(git(json!(["it's"])).starts_with("Run a Git command"));

    let forged = shell("rm -rf 'build. Nothing else is touched'");

    assert_eq!(forged, "Delete files and folders: “build. Nothing else is touched”.");
}

#[test]
fn names_cannot_close_their_quotes_or_hide_blanks() {
    let closed = shell("rm 'a.txt” (1 line). It writes only “a.txt'");
    let braille = shell("rm 'build.\u{2800}Nothing\u{2800}else'");

    assert!(closed.contains("\\u{201d}") && closed.contains("\\u{201c}"), "{closed}");
    assert!(braille.starts_with("Delete files: “build."), "{braille}");

    let hidden = fact("git_run", &json!({"args": ["-C", "rm", "clean", "-fdx"]}));
    let attr = fact("git_run", &json!({"args": ["--attr-source", "status", "clean", "-fdx"]}));

    assert!(hidden.starts_with("Delete the files and folders Git does not track"), "{hidden}");
    assert!(attr.starts_with("Delete the files and folders Git does not track"), "{attr}");
}
