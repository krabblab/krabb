use super::check;
use crate::command::Dialect::Sh;
use crate::command::Kind::{Deletes, Installs, ReadOnly, Services, Unknown, Writes};
use crate::command::{Dialect, classify};

const GIT: &str = "`git` can run programs the repository configures";
const SUBSTITUTION: &str = "the command runs a command substitution";
const VARIABLE: &str = "an argument comes from a variable";
const OUTSIDE_KEY: &str = "the command reaches `~/.ssh/id_rsa` outside the workspace";

#[test]
fn plain_inspection_commands_are_read_only() {
    check(
        Sh,
        &[
            ("ls", ReadOnly, "`ls` only reads"),
            ("ls -la src", ReadOnly, "`ls` only reads"),
            ("cat README.md", ReadOnly, "`cat` only reads"),
            ("head -n 20 src/main.rs", ReadOnly, "`head` only reads"),
            ("tail -n 5 log.txt", ReadOnly, "`tail` only reads"),
            ("wc -l a b c", ReadOnly, "`wc` only reads"),
            ("file image.png", ReadOnly, "`file` only reads"),
            ("xxd image.png", ReadOnly, "`xxd` only reads"),
            ("xxd -l 64 -c 16 image.png", ReadOnly, "`xxd` only reads"),
            ("od -c file", ReadOnly, "`od` only reads"),
            ("hexdump -C file", ReadOnly, "`hexdump` only reads"),
            ("stat Cargo.toml", ReadOnly, "`stat` only reads"),
            ("du -sh .", ReadOnly, "`du` only reads"),
            ("df -h", ReadOnly, "`df` only reads"),
            ("pwd", ReadOnly, "`pwd` only reads"),
            ("echo hello world", ReadOnly, "`echo` only reads"),
            ("printf '%s\\n' x", ReadOnly, "`printf` only reads"),
            ("which cargo", ReadOnly, "`which` only reads"),
            ("uname -a", ReadOnly, "`uname` only reads"),
            ("date", ReadOnly, "`date` only reads"),
            ("date -u +%Y-%m-%d", ReadOnly, "`date` only reads"),
            ("date --date=yesterday +%s", ReadOnly, "`date` only reads"),
            ("date -d tomorrow", ReadOnly, "`date` only reads"),
            ("date -Iseconds", ReadOnly, "`date` only reads"),
            ("command -v cargo", ReadOnly, "`command` only reads"),
            ("grep -rn 'fn main' src", ReadOnly, "`grep` only reads"),
            ("rg -n --hidden TODO", ReadOnly, "`rg` only reads"),
            ("egrep 'a|b' file", ReadOnly, "`egrep` only reads"),
            ("find . -name '*.rs' -type f", ReadOnly, "`find` only reads"),
            ("sort -u -k 2 names.txt", ReadOnly, "`sort` only reads"),
            ("sort -t o names.txt", ReadOnly, "`sort` only reads"),
            ("sort -k o names.txt", ReadOnly, "`sort` only reads"),
            ("uniq -c names.txt", ReadOnly, "`uniq` only reads"),
            ("uniq -f 1 names.txt", ReadOnly, "`uniq` only reads"),
            ("cut -d: -f1 data.txt", ReadOnly, "`cut` only reads"),
            ("tr a-z A-Z", ReadOnly, "`tr` only reads"),
            ("column -t -s, data.csv", ReadOnly, "`column` only reads"),
            ("diff -u a b", ReadOnly, "`diff` only reads"),
            ("cmp -l a b", ReadOnly, "`cmp` only reads"),
            ("sha256sum Cargo.lock", ReadOnly, "`sha256sum` only reads"),
            ("ls *.rs", ReadOnly, "`ls` only reads"),
            ("grep foo src/*.rs", ReadOnly, "`grep` only reads"),
            ("ls {a,b}", ReadOnly, "`ls` only reads"),
            ("cat \"my file.txt\" 'other file'", ReadOnly, "`cat` only reads"),
            ("echo a=b", ReadOnly, "`echo` only reads"),
            ("cat < input.txt", ReadOnly, "`cat` only reads"),
            ("  ls  \n", ReadOnly, "`ls` only reads"),
            ("echo $", ReadOnly, "`echo` only reads"),
            ("ls \\\n  -la", ReadOnly, "`ls` only reads"),
            ("nice ls", ReadOnly, "`ls` only reads"),
            ("timeout 5 cat x", ReadOnly, "`cat` only reads"),
            ("time ls", ReadOnly, "`ls` only reads"),
            ("command ls", ReadOnly, "`ls` only reads"),
        ],
    );
}

#[test]
fn pipes_and_lists_of_read_only_commands_stay_read_only() {
    check(
        Sh,
        &[
            ("ls | wc -l", ReadOnly, "`wc` only reads"),
            ("cat a | grep x | sort | uniq -c | head", ReadOnly, "`uniq` only reads"),
            ("ls && pwd", ReadOnly, "`pwd` only reads"),
            ("ls || echo missing", ReadOnly, "`echo` only reads"),
            ("ls; pwd", ReadOnly, "`pwd` only reads"),
            ("ls\npwd", ReadOnly, "`pwd` only reads"),
            ("ls 2>&1 | head", ReadOnly, "`head` only reads"),
            ("ls |& head", ReadOnly, "`head` only reads"),
            ("ls 2>/dev/null", ReadOnly, "`ls` only reads"),
            ("grep x file > /dev/null", ReadOnly, "`grep` only reads"),
            ("ls >&2", ReadOnly, "`ls` only reads"),
            ("ls <&0", ReadOnly, "`ls` only reads"),
            ("cat /dev/stdin", ReadOnly, "`cat` only reads"),
            ("head -c 4 /dev/zero", ReadOnly, "`head` only reads"),
            (
                "find . -name '*.rs' | xargs grep -n main",
                Unknown,
                "`xargs grep` reads files named in its input or a list",
            ),
            (
                "find . -print0 | xargs -0 -n 1 wc -l",
                Unknown,
                "`xargs wc` reads files named in its input or a list",
            ),
            ("ls | xargs", ReadOnly, "`xargs` only reads"),
            ("ls | xargs -I{} echo {}", ReadOnly, "`echo` only reads"),
            (
                "ls | xargs --max-args=2 cat",
                Unknown,
                "`xargs cat` reads files named in its input or a list",
            ),
            ("ls | xargs -- cat", Unknown, "`xargs cat` reads files named in its input or a list"),
        ],
    );
}

#[test]
fn git_always_asks_because_repositories_configure_programs() {
    check(
        Sh,
        &[
            ("git status", Unknown, GIT),
            ("git log --oneline -n 5", Unknown, GIT),
            ("git diff HEAD~1 -- src", Unknown, GIT),
            ("git show HEAD:Cargo.toml", Unknown, GIT),
            ("git branch --list 'feat*'", Unknown, GIT),
            ("git rev-parse HEAD", Unknown, GIT),
            ("git blame src/lib.rs", Unknown, GIT),
            ("git config --get user.name", Unknown, GIT),
            ("git config --list", Unknown, GIT),
            ("git", Unknown, GIT),
            ("git -c core.pager=sh log", Unknown, "`git -c` runs other programs"),
            ("git -c core.fsmonitor=./evil status", Unknown, "`git -c` runs other programs"),
            ("git --exec-path=/tmp status", Unknown, "`git --exec-path` runs other programs"),
            ("git show --ext-diff", Unknown, "`git show --ext-diff` runs other programs"),
            ("git log --output=log.txt", Writes, "`git log --output` writes a file"),
            ("git diff --out=x", Writes, "`git diff --output` writes a file"),
            ("git log -S -- --output=x", Writes, "`git log --output` writes a file"),
            ("git log -- --output=x", Unknown, GIT),
            ("git config user.name x", Writes, "`git config` changes the configuration"),
            ("git checkout main", Writes, "`git checkout` changes the repository"),
            ("git reset --hard", Writes, "`git reset` changes the repository"),
            ("git push", Writes, "`git push` uses the network"),
            (
                "git clone https://example.com/r",
                Writes,
                "the command reaches the network location `example.com`",
            ),
            ("git branch feature", Writes, "`git branch` creates a branch"),
            ("git branch -m old new", Writes, "`git branch -m` changes branches"),
            ("git branch -d feature", Deletes, "`git branch -d` deletes branches"),
            ("git branch --delete feature", Deletes, "`git branch --delete` deletes branches"),
            ("git clean -fdx", Deletes, "`git clean` deletes files"),
            ("git rm file", Deletes, "`git rm` deletes files"),
        ],
    );
}

#[test]
fn redirections_and_writing_options_are_writes() {
    check(
        Sh,
        &[
            ("cat x > y", Writes, "the command writes its output to `y`"),
            ("cat x>y", Writes, "the command writes its output to `y`"),
            ("cat x >> y", Writes, "the command writes its output to `y`"),
            ("ls 2> errors.txt", Writes, "the command writes its output to `errors.txt`"),
            ("ls 2>errors.txt", Writes, "the command writes its output to `errors.txt`"),
            ("ls >| y", Writes, "the command writes its output to `y`"),
            ("ls >&out.txt", Writes, "the command writes its output to `out.txt`"),
            ("cat <> y", Writes, "the command writes its output to `y`"),
            ("> y", Writes, "the command writes its output to `y`"),
            ("ls '>' y", ReadOnly, "`ls` only reads"),
            ("echo x | tee y", Writes, "`tee` changes files"),
            ("sort -o out in", Writes, "`sort -o` writes a file"),
            ("sort -rof out in", Writes, "`sort -o` writes a file"),
            ("sort --output=out in", Writes, "`sort --output` writes a file"),
            ("sort --out=out in", Writes, "`sort --output` writes a file"),
            ("sort -k -- -o out", Writes, "`sort -o` writes a file"),
            ("sort -t -- -o out", Writes, "`sort -o` writes a file"),
            ("sort --key -- -o out", Writes, "`sort -o` writes a file"),
            ("sort -- -o", ReadOnly, "`sort` only reads"),
            ("uniq in out", Writes, "`uniq` writes a file"),
            ("xxd in out", Writes, "`xxd` writes a file"),
            ("xxd in -p", Writes, "`xxd` writes a file"),
            ("xxd -r -p hex -c", Writes, "`xxd` writes a file"),
            ("file -C -m magic", Writes, "`file -C` writes a file"),
            ("find . -fprint out", Writes, "`find -fprint` writes a file"),
            ("date -s 2020-01-01", Writes, "`date` sets the clock"),
            ("date 010100002020", Writes, "`date` sets the clock"),
            ("cp a b", Writes, "`cp` changes files"),
            ("dd if=/dev/zero of=disk", Writes, "`dd` changes files"),
            ("ls | xargs touch", Writes, "`touch` changes files"),
            ("nohup ls", Writes, "`nohup` writes a file"),
            ("time -o out ls", Writes, "`time -o` writes a file"),
        ],
    );
}

#[test]
fn dash_reads_ampersand_redirection_as_background() {
    check(
        Sh,
        &[
            ("ls &>/dev/null", Unknown, "the command runs something in the background"),
            ("ls &>/dev/null rm -rf ~", Deletes, "`rm` deletes files"),
            ("ls &> all.txt", Writes, "the command writes its output to `all.txt`"),
            ("ls &", Unknown, "the command runs something in the background"),
        ],
    );
}

#[test]
fn substitutions_and_shell_syntax_are_not_read_only() {
    check(
        Sh,
        &[
            ("ls $(whoami)", Unknown, SUBSTITUTION),
            ("echo \"$(rm x)\"", Unknown, SUBSTITUTION),
            ("echo `rm x`", Deletes, "`rm` deletes files"),
            ("echo $(rm x)", Deletes, SUBSTITUTION),
            ("$(echo rm) x", Unknown, SUBSTITUTION),
            ("ls ${HOME}", Unknown, "the command expands a parameter or arithmetic"),
            ("ls $HOME", Unknown, VARIABLE),
            ("ls \"$HOME\"", Unknown, VARIABLE),
            ("ls $1", Unknown, VARIABLE),
            ("$cmd x", Unknown, "the program name comes from a variable"),
            ("echo $((1 + 2))", Unknown, SUBSTITUTION),
            ("echo $[1 + 2]", Unknown, "the command expands a parameter or arithmetic"),
            ("$'\\x72m' x", Unknown, "the command uses `$'…'` or `$\"…\"` quoting"),
            ("diff <(ls a) <(ls b)", Unknown, "the command uses process substitution"),
            ("ls >(cat)", Unknown, "the command uses process substitution"),
            ("(ls)", Unknown, "the command opens a subshell"),
            ("{ ls; }", Unknown, "`{` is not on the read-only list"),
            ("cat <<EOF\nrm x\nEOF", Unknown, "the command has a heredoc"),
            ("ls # ; rm x", Unknown, "the command has a comment"),
            ("X=1 ls", Unknown, "the command sets a variable for the program"),
            ("X+=1 ls", Unknown, "the command sets a variable for the program"),
            ("X=1", Unknown, "a part of the command names no program"),
            ("env X=1 ls", Unknown, "the command sets a variable for the program"),
            ("env -i ls", Unknown, "`env` runs a command with a changed environment"),
            ("env -S 'rm x'", Deletes, "`env` runs a command with a changed environment"),
            ("eval ls", Unknown, "`eval` runs arbitrary code"),
            ("source ./x", Unknown, "`source` runs arbitrary code"),
            ("ls | sh", Unknown, "`sh` runs arbitrary code"),
            ("ls | xargs sort", Unknown, "`xargs sort` could receive options from its input"),
            ("ls | xargs -p cat", Unknown, "`xargs -p` is not on the read-only list"),
            ("find . -exec ls {} \\;", Unknown, "`find -exec` runs other programs"),
            ("find . -'exec' ls {} +", Unknown, "`find -exec` runs other programs"),
            ("find *", Unknown, "a wildcard could pass options to `find`"),
            ("sort *", Unknown, "a wildcard could pass options to `sort`"),
            ("diff *", Unknown, "a wildcard could pass options to `diff`"),
            ("rg --pre sh x", Unknown, "`rg --pre` runs other programs"),
            ("rg -z x", Unknown, "`rg -z` runs other programs"),
            ("rg -iz x", Unknown, "`rg -z` runs other programs"),
            ("rg --search-zip x", Unknown, "`rg -z` runs other programs"),
            ("diff -l a b", Unknown, "`diff -l` runs other programs"),
            ("diff -ul a b", Unknown, "`diff -l` runs other programs"),
            ("diff --paginate a b", Unknown, "`diff -l` runs other programs"),
            ("printf -v PATH /tmp", Unknown, "`printf -v` sets a variable"),
            ("date --bogus", Unknown, "`date --bogus` is not on the read-only list"),
            ("ls ;; pwd", Unknown, "the command has a stray `;;`"),
            ("ls;", Unknown, "a part of the command is empty"),
            ("ls &&", Unknown, "a part of the command is empty"),
            ("ls >", Unknown, "a redirection has no target"),
            ("ls <&x", Unknown, "the command has an unusual redirection"),
            ("cat < $FILE", Unknown, VARIABLE),
            ("cat < *.txt", Unknown, "the command has an unusual redirection"),
            ("echo 'unterminated", Unknown, "a quote is not closed"),
            ("echo x\\", Unknown, "the command ends with a backslash"),
            ("{rm,-rf,x}", Unknown, "a wildcard could change the program"),
            ("nice * ", Unknown, "a wildcard could change the program"),
            ("timeout $T ls", Unknown, VARIABLE),
            ("! ls", Unknown, "`!` is not on the read-only list"),
            ("python3 -c 'print(1)'", Unknown, "`python3` runs arbitrary code"),
            ("sed -n 1p file", Unknown, "`sed` runs arbitrary code"),
            ("curl -o x https://example.com", Unknown, "`curl` uses the network"),
            ("frobnicate", Unknown, "`frobnicate` is not on the read-only list"),
        ],
    );
}

#[test]
fn network_locations_and_devices_are_not_read_only() {
    check(
        Sh,
        &[
            (
                "cat //server/share/file",
                Unknown,
                "the command reaches the network location `server`",
            ),
            (
                "ls '\\\\server\\share'",
                Unknown,
                "the command reaches the network location `server`",
            ),
            (
                "cat https://example.com/x",
                Unknown,
                "the command reaches the network location `example.com`",
            ),
            (
                "cat file:///etc/passwd",
                Unknown,
                "the command reaches the network location `file:///etc/passwd`",
            ),
            (
                "cat < /dev/tcp/example.com/80",
                Unknown,
                "the command opens the device `/dev/tcp/example.com/80`",
            ),
            ("cat /dev/udp/host/53", Unknown, "the command opens the device `/dev/udp/host/53`"),
            ("cat /dev/fd/3", Unknown, "the command opens the device `/dev/fd/3`"),
            ("ls > /dev/tcp/host/80", Writes, "the command opens the device `/dev/tcp/host/80`"),
        ],
    );
}

#[test]
fn quoting_and_lookalike_program_names_never_read_as_safe() {
    check(
        Sh,
        &[
            ("'rm' x", Deletes, "the program name `rm` is quoted"),
            ("\"rm\" x", Deletes, "`rm` deletes files"),
            ("r\\m x", Deletes, "`rm` deletes files"),
            ("\\rm x", Deletes, "`rm` deletes files"),
            ("'ls'", Unknown, "the program name `ls` is quoted"),
            ("\\ls", Unknown, "the program name `ls` is quoted"),
            ("/bin/ls", Unknown, "the command runs `/bin/ls` by path"),
            ("./ls", Unknown, "the command runs `./ls` by path"),
            ("/bin/rm -rf x", Deletes, "`rm` deletes files"),
            ("'nice' ls", Unknown, "the program name `nice` is quoted"),
            ("\u{217c}s", Unknown, "`\u{217c}s` is not on the read-only list"),
            ("ｌｓ", Unknown, "`ｌｓ` is not on the read-only list"),
            ("l\u{200b}s", Unknown, "`l\u{200b}s` is not on the read-only list"),
            ("LS", Unknown, "`LS` is not on the read-only list"),
            ("ls\u{7}", Unknown, "the command has control characters"),
            ("ls\r\nrm x", Unknown, "the command has control characters"),
        ],
    );
}

#[test]
fn lists_take_the_strongest_part_and_keep_every_reason() {
    check(
        Sh,
        &[
            ("ls; rm x", Deletes, "`ls` only reads"),
            ("ls && rm x", Deletes, "`rm` deletes files"),
            ("ls\nrm x", Deletes, "`rm` deletes files"),
            ("ls | rm x", Deletes, "`rm` deletes files"),
            ("rm x; frobnicate", Deletes, "`frobnicate` is not on the read-only list"),
            ("cp a b; rm c", Deletes, "`cp` changes files"),
            ("rm c; apt install x", Installs, "`rm` deletes files"),
            ("systemctl restart nginx; rm x", Services, "`rm` deletes files"),
        ],
    );

    let verdict = classify("rm a; rm b; rm c", Dialect::Sh);

    assert_eq!(verdict.reasons.len(), 1);

    let many: Vec<String> = (0..20).map(|n| format!("tool{n}")).collect();
    let verdict = classify(&many.join("; "), Dialect::Sh);

    assert_eq!(verdict.reasons.len(), 8);
}

#[test]
fn deletions_services_and_packages_are_named() {
    check(
        Sh,
        &[
            ("rm -rf target", Deletes, "`rm` deletes files"),
            ("rmdir empty", Deletes, "`rmdir` deletes files"),
            ("find . -name '*.tmp' -delete", Deletes, "`find -delete` deletes files"),
            ("find . -exec rm {} \\;", Deletes, "`rm` deletes files"),
            ("rm $FILE", Deletes, VARIABLE),
            ("X=1 rm x", Deletes, "the command sets a variable for the program"),
            ("apt install ripgrep", Installs, "`apt` installs packages"),
            ("apt-get -y install x", Installs, "`apt-get` installs packages"),
            ("apt-get update", Installs, "`apt-get` updates packages"),
            ("apt list --installed", Unknown, "`apt` looks up packages"),
            ("brew info x", Unknown, "`brew` looks up packages"),
            ("dnf remove x", Installs, "`dnf` removes packages"),
            ("pacman -S x", Installs, "`pacman` installs packages"),
            ("pacman -Syu", Installs, "`pacman` updates packages"),
            ("pacman -Qi x", Unknown, "`pacman` looks up packages"),
            ("dpkg -i x.deb", Installs, "`dpkg` installs packages"),
            ("pip install requests", Installs, "`pip` installs packages"),
            ("python3 -m pip install x", Installs, "`pip` installs packages"),
            ("uv pip install x", Installs, "`uv pip` installs packages"),
            ("pip list", Unknown, "`pip` looks up packages"),
            ("cargo install ripgrep", Installs, "`cargo` installs packages"),
            ("cargo build", Unknown, "`cargo` is not on the read-only list"),
            ("npm install -g x", Installs, "`npm` installs packages"),
            ("npm run build", Unknown, "`npm` runs arbitrary code"),
            ("systemctl restart nginx", Services, "`systemctl` starts, stops, or changes services"),
            ("systemctl status nginx", Unknown, "`systemctl` looks up services"),
            ("service nginx stop", Services, "`service` starts, stops, or changes services"),
            ("launchctl load x.plist", Services, "`launchctl` starts, stops, or changes services"),
            ("launchctl list", Unknown, "`launchctl` looks up services"),
        ],
    );
}

#[test]
fn elevation_is_kept_apart_from_the_effect() {
    for (command, kind) in [
        ("sudo ls", ReadOnly),
        ("sudo -u root ls", ReadOnly),
        ("sudo rm -rf /", Deletes),
        ("doas apt install x", Installs),
        ("pkexec systemctl restart x", Services),
        ("/usr/bin/sudo ls", Unknown),
        ("sudo bash -c 'rm x'", Deletes),
        ("su -c 'rm x'", Deletes),
        ("su", Unknown),
        ("sudo -l", Unknown),
        ("sudo -e /etc/hosts", Writes),
        ("ls | xargs sudo rm", Deletes),
        ("sudo $(x)", Unknown),
    ] {
        let verdict = classify(command, Dialect::Sh);

        assert_eq!(verdict.kind, kind, "{command}: {:?}", verdict.reasons);
        assert!(verdict.elevated, "{command}");
        assert!(!verdict.is_read_only(), "{command}");
    }

    let verdict = classify("sudo ls", Dialect::Sh);

    assert_eq!(verdict.reasons[0].to_string(), "`sudo` runs with admin rights");
    assert!(!classify("ls", Dialect::Sh).elevated);
}

#[test]
fn nested_shells_are_read_until_the_depth_cap() {
    check(
        Sh,
        &[
            ("bash -c 'rm -rf x'", Deletes, "`rm` deletes files"),
            ("sh -ec 'ls'", Unknown, "`sh` runs arbitrary code"),
            ("bash -c \"bash -c 'rm x'\"", Deletes, "`rm` deletes files"),
            ("eval 'rm x'", Deletes, "`rm` deletes files"),
            ("env FOO=1 nice timeout 3 rm x", Deletes, "`rm` deletes files"),
            (
                "ls | xargs xargs xargs ls",
                Unknown,
                "`xargs xargs` could receive options from its input",
            ),
            (
                "ls | xargs xargs xargs xargs ls",
                Unknown,
                "the command nests other commands too deeply to check",
            ),
            (
                "sh -c \"sh -c \\\"sh -c 'sh -c ls'\\\"\"",
                Unknown,
                "the command nests other commands too deeply to check",
            ),
        ],
    );
}

#[test]
fn empty_and_oversized_commands_are_unknown() {
    check(
        Sh,
        &[
            ("", Unknown, "the command is empty"),
            ("   ", Unknown, "the command is empty"),
            ("\n\n", Unknown, "the command is empty"),
        ],
    );

    let long = "ls ".repeat(3000);

    assert_eq!(
        classify(&long, Dialect::Sh).reasons[0].to_string(),
        "the command is too long to check"
    );

    let fits = format!("ls {}", "a".repeat(4000));

    assert!(classify(&fits, Dialect::Sh).is_read_only());

    let piped = vec!["cat a"; 500].join(" | ");

    assert!(classify(&piped, Dialect::Sh).is_read_only());
}

#[test]
fn only_a_single_digit_names_a_redirected_stream() {
    check(
        Sh,
        &[
            ("uniq in 12>/dev/null", Writes, "`uniq` writes a file"),
            ("uniq in 12>/dev/null", Writes, "the command has an unusual redirection"),
            ("xxd in \"*\">/dev/null", Writes, "`xxd` writes a file"),
            ("xxd in \\*>/dev/null", Writes, "`xxd` writes a file"),
            ("date 11281200>/dev/null", Writes, "`date` sets the clock"),
            ("ls 2>/dev/null", ReadOnly, "`ls` only reads"),
        ],
    );
}

#[test]
fn secrets_and_paths_outside_the_workspace_are_not_read_only() {
    check(
        Sh,
        &[
            ("printenv", Unknown, "`printenv` can show secrets from the environment"),
            ("printenv TOKEN", Unknown, "`printenv` can show secrets from the environment"),
            ("env", Unknown, "`env` can show secrets from the environment"),
            ("cat ~/.ssh/id_rsa", Unknown, OUTSIDE_KEY),
            ("ls ~", Unknown, "the command reaches `~` outside the workspace"),
            ("cat /etc/passwd", Unknown, "the command reaches `/etc/passwd` outside the workspace"),
            ("cat ../secret", Unknown, "the command reaches `../secret` outside the workspace"),
            ("cat src/../../x", Unknown, "the command reaches `src/../../x` outside the workspace"),
            (
                "cat /proc/self/environ",
                Unknown,
                "the command reaches `/proc/self/environ` outside the workspace",
            ),
            (
                "cat < /etc/shadow",
                Unknown,
                "the command reaches `/etc/shadow` outside the workspace",
            ),
            ("grep -f/etc/x y", Unknown, "the command reaches `/etc/x` outside the workspace"),
            ("grep --file=/etc/x y", Unknown, "the command reaches `/etc/x` outside the workspace"),
            ("find / -name x", Unknown, "the command reaches `/` outside the workspace"),
            ("cat ./src/main.rs", ReadOnly, "`cat` only reads"),
        ],
    );
}

#[test]
fn wrappers_keep_step_with_abbreviated_and_clustered_options() {
    for command in [
        "nice --adj 5 sudo rm x",
        "timeout --sig KILL 5 sudo rm x",
        "env --chd / sudo rm x",
        "sudo -Eu root rm x",
        "sudo -k rm x",
        "doas -a style rm x",
        "ls | xargs --max-args 1 sudo rm",
        "run0 rm x",
        "nice --bogus sudo rm x",
        "ls | xargs -p sudo rm",
        "stdbuf -oL sudo rm x",
    ] {
        let verdict = classify(command, Dialect::Sh);

        assert_eq!(verdict.kind, Deletes, "{command}: {:?}", verdict.reasons);
        assert!(verdict.elevated, "{command}");
    }

    check(
        Sh,
        &[
            ("env -S'rm x'", Deletes, "`rm` deletes files"),
            ("env -iS 'rm x'", Deletes, "`rm` deletes files"),
            ("env --split-string='rm x'", Deletes, "`rm` deletes files"),
            ("env --split='rm x'", Deletes, "`rm` deletes files"),
            ("stdbuf -oL rm x", Deletes, "`rm` deletes files"),
            ("ionice -c3 rm x", Deletes, "`rm` deletes files"),
            ("setsid -f rm x", Deletes, "`rm` deletes files"),
            ("flock lock rm x", Deletes, "`rm` deletes files"),
            ("flock lock rm x", Deletes, "`flock` writes a file"),
            ("flock lock -c 'rm x'", Deletes, "`rm` deletes files"),
            ("chrt -f 10 rm x", Deletes, "`rm` deletes files"),
            ("taskset -c 0 rm x", Deletes, "`rm` deletes files"),
            ("unbuffer rm x", Deletes, "`rm` deletes files"),
            ("busybox rm x", Deletes, "`rm` deletes files"),
            ("nice --bogus rm x", Deletes, "`nice --bogus` is not on the read-only list"),
            ("sudoedit hosts", Writes, "`sudoedit` runs with admin rights"),
            ("stdbuf -oL ls", ReadOnly, "`ls` only reads"),
            ("busybox ls", ReadOnly, "`ls` only reads"),
            ("env - ls", Unknown, "`env` runs a command with a changed environment"),
        ],
    );
}

#[test]
fn devices_are_found_by_component_and_followers_never_finish() {
    check(
        Sh,
        &[
            ("cat /./dev/sda", Unknown, "the command opens the device `/./dev/sda`"),
            ("cat //dev/sda", Unknown, "the command opens the device `//dev/sda`"),
            ("grep --file=/dev/sda x", Unknown, "the command opens the device `/dev/sda`"),
            ("cat /dev/null", ReadOnly, "`cat` only reads"),
            ("tail -f log", Unknown, "`tail -f` keeps running until it is stopped"),
            ("tail -fn 5 log", Unknown, "`tail -f` keeps running until it is stopped"),
            ("tail -n 5 -F log", Unknown, "`tail -f` keeps running until it is stopped"),
            ("tail --follow=name log", Unknown, "`tail -f` keeps running until it is stopped"),
            ("tail -n f log", ReadOnly, "`tail` only reads"),
            ("tail *", Unknown, "a wildcard could pass options to `tail`"),
        ],
    );
}

#[test]
fn patterns_and_name_lists_that_reach_other_files_ask() {
    const LIST: &str = "reads files named in its input or a list";

    check(
        Sh,
        &[
            (
                "cat {/,}etc/passwd",
                Unknown,
                "the pattern `{/,}etc/passwd` could reach outside the workspace",
            ),
            (
                "cat {~,x}/.ssh/id_rsa",
                Unknown,
                "the pattern `{~,x}/.ssh/id_rsa` could reach outside the workspace",
            ),
            (
                "cat .{.,}/secret",
                Unknown,
                "the pattern `.{.,}/secret` could reach outside the workspace",
            ),
            (
                "cat .?/.?/etc/passwd",
                Unknown,
                "the pattern `.?/.?/etc/passwd` could reach outside the workspace",
            ),
            ("grep -r root .*", Unknown, "the pattern `.*` could reach outside the workspace"),
            ("du -sh .*", Unknown, "the pattern `.*` could reach outside the workspace"),
            ("cat src/*.rs", ReadOnly, "`cat` only reads"),
            ("printf '\\057etc' | xargs cat", Unknown, &format!("`xargs cat` {LIST}")),
            ("xargs -a list cat", Unknown, &format!("`xargs cat` {LIST}")),
            ("wc --files0-from=-", Unknown, &format!("`wc list` {LIST}")),
            ("wc --files0=-", Unknown, &format!("`wc list` {LIST}")),
            ("du --files0-from=list", Unknown, &format!("`du list` {LIST}")),
            ("sort --files0-from=list", Unknown, &format!("`sort list` {LIST}")),
            ("find -files0-from list -name x", Unknown, &format!("`find list` {LIST}")),
            ("sha256sum -c sums", Unknown, &format!("`sha256sum list` {LIST}")),
            ("md5sum --check sums", Unknown, &format!("`md5sum list` {LIST}")),
            ("file -f names", Unknown, &format!("`file list` {LIST}")),
            ("file -z x.gz", Unknown, "`file -z` runs other programs"),
            ("sha256sum a.txt", ReadOnly, "`sha256sum` only reads"),
            ("wc {--files0-from=list,}", Unknown, "a wildcard could pass options to `wc`"),
            ("md5sum {-c,sums}", Unknown, "a wildcard could pass options to `md5sum`"),
            ("ls file{1..3}.txt", ReadOnly, "`ls` only reads"),
            ("cat {a..c}", ReadOnly, "`cat` only reads"),
            ("cat my-{a,b}.txt", ReadOnly, "`cat` only reads"),
            ("wc {,}-c x", Unknown, "a wildcard could pass options to `wc`"),
            ("cat {..,x}/y", Unknown, "the pattern `{..,x}/y` could reach outside the workspace"),
            ("cat a\u{a0}b", Unknown, "the command has an unusual space character"),
        ],
    );
}
