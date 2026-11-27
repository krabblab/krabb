use super::Kind::{Deletes, Elevates, Installs, ReadOnly, Unknown, Writes};
use super::{Dialect, Kind, classify};

fn check(dialect: Dialect, cases: &[(&str, Kind)]) {
    for (command, kind) in cases {
        let verdict = classify(command, dialect);

        assert_eq!(verdict.kind, *kind, "{dialect:?} {command:?}: {}", verdict.reason);
        assert!(!verdict.reason.is_empty(), "{command:?}");
    }
}

#[test]
fn plain_inspection_commands_are_read_only() {
    check(
        Dialect::Sh,
        &[
            ("ls", ReadOnly),
            ("ls -la /tmp", ReadOnly),
            ("cat README.md", ReadOnly),
            ("head -n 20 src/main.rs", ReadOnly),
            ("tail -f log.txt", ReadOnly),
            ("wc -l a b c", ReadOnly),
            ("file image.png", ReadOnly),
            ("xxd image.png", ReadOnly),
            ("xxd -l 64 -c 16 image.png", ReadOnly),
            ("od -c file", ReadOnly),
            ("hexdump -C file", ReadOnly),
            ("stat Cargo.toml", ReadOnly),
            ("du -sh .", ReadOnly),
            ("df -h", ReadOnly),
            ("pwd", ReadOnly),
            ("echo hello world", ReadOnly),
            ("printf '%s\\n' x", ReadOnly),
            ("which cargo", ReadOnly),
            ("uname -a", ReadOnly),
            ("date", ReadOnly),
            ("date -u +%Y-%m-%d", ReadOnly),
            ("date --date=yesterday +%s", ReadOnly),
            ("date -d tomorrow", ReadOnly),
            ("date -Iseconds", ReadOnly),
            ("env", ReadOnly),
            ("printenv PATH", ReadOnly),
            ("grep -rn 'fn main' src", ReadOnly),
            ("rg -n --hidden TODO", ReadOnly),
            ("egrep 'a|b' file", ReadOnly),
            ("fgrep -i x file", ReadOnly),
            ("find . -name '*.rs' -type f", ReadOnly),
            ("find src -maxdepth 2 -print", ReadOnly),
            ("sort -u -k 2 names.txt", ReadOnly),
            ("sort -t o names.txt", ReadOnly),
            ("uniq -c names.txt", ReadOnly),
            ("uniq -f 1 names.txt", ReadOnly),
            ("cut -d: -f1 /etc/passwd", ReadOnly),
            ("tr a-z A-Z", ReadOnly),
            ("column -t -s, data.csv", ReadOnly),
            ("diff -u a b", ReadOnly),
            ("cmp a b", ReadOnly),
            ("sha256sum Cargo.lock", ReadOnly),
            ("md5sum a b", ReadOnly),
            ("ls *.rs", ReadOnly),
            ("grep foo src/*.rs", ReadOnly),
            ("cat \"my file.txt\" 'other file'", ReadOnly),
            ("echo a=b", ReadOnly),
            ("ls ~/projects", ReadOnly),
            ("cat < input.txt", ReadOnly),
            ("  ls  \n", ReadOnly),
            ("echo $", ReadOnly),
            ("ls \\\n  -la", ReadOnly),
        ],
    );
}

#[test]
fn git_inspection_is_read_only_and_changes_are_classified() {
    check(
        Dialect::Sh,
        &[
            ("git status", ReadOnly),
            ("git status --short", ReadOnly),
            ("git log --oneline -n 5", ReadOnly),
            ("git diff HEAD~1 -- src", ReadOnly),
            ("git show HEAD:Cargo.toml", ReadOnly),
            ("git branch", ReadOnly),
            ("git branch --list 'feat*'", ReadOnly),
            ("git branch -av", ReadOnly),
            ("git branch --contains HEAD", ReadOnly),
            ("git branch --show-current", ReadOnly),
            ("git rev-parse HEAD", ReadOnly),
            ("git ls-files", ReadOnly),
            ("git blame src/lib.rs", ReadOnly),
            ("git -C repo status", ReadOnly),
            ("git --no-pager log", ReadOnly),
            ("git -c core.pager=sh log", Unknown),
            ("git -c core.fsmonitor=evil status", Unknown),
            ("git --git-dir=/tmp/x status", Unknown),
            ("git -p log", Unknown),
            ("git", Unknown),
            ("git -C", Unknown),
            ("git lfs ls-files", Unknown),
            ("git st", Unknown),
            ("git log --output=log.txt", Writes),
            ("git diff --out=x", Writes),
            ("git show --ext-diff", Unknown),
            ("git log -- --output=x", ReadOnly),
            ("git config user.name x", Writes),
            ("git config --get user.name", Writes),
            ("git checkout main", Writes),
            ("git reset --hard", Writes),
            ("git commit -m x", Writes),
            ("git push", Writes),
            ("git branch feature", Writes),
            ("git branch -m old new", Writes),
            ("git branch --set-upstream-to=origin/main", Writes),
            ("git branch -", Writes),
            ("git branch -d feature", Deletes),
            ("git branch -D feature", Deletes),
            ("git branch --delete feature", Deletes),
            ("git clean -fdx", Deletes),
            ("git rm file", Deletes),
        ],
    );
}

#[test]
fn pipes_and_lists_of_read_only_commands_stay_read_only() {
    check(
        Dialect::Sh,
        &[
            ("ls | wc -l", ReadOnly),
            ("cat a | grep x | sort | uniq -c | head", ReadOnly),
            ("git log --oneline | head -n 3", ReadOnly),
            ("ls && pwd", ReadOnly),
            ("ls || echo missing", ReadOnly),
            ("ls; pwd", ReadOnly),
            ("ls\npwd", ReadOnly),
            ("ls 2>&1 | head", ReadOnly),
            ("ls |& head", ReadOnly),
            ("ls 2>/dev/null", ReadOnly),
            ("grep x file > /dev/null", ReadOnly),
            ("ls &>/dev/null", ReadOnly),
            ("ls >&2", ReadOnly),
            ("ls <&0", ReadOnly),
            ("find . -name '*.rs' | xargs grep -n main", ReadOnly),
            ("find . -print0 | xargs -0 -n 1 wc -l", ReadOnly),
            ("ls | xargs", ReadOnly),
            ("ls | xargs -I{} echo {}", ReadOnly),
            ("ls | xargs --max-args=2 cat", ReadOnly),
            ("ls | xargs -- cat", ReadOnly),
        ],
    );
}

#[test]
fn redirections_into_files_are_writes() {
    check(
        Dialect::Sh,
        &[
            ("cat x > y", Writes),
            ("cat x>y", Writes),
            ("cat x >> y", Writes),
            ("ls 2> errors.txt", Writes),
            ("ls 2>errors.txt", Writes),
            ("ls &> all.txt", Writes),
            ("ls &>> all.txt", Writes),
            ("ls >| y", Writes),
            ("ls >&out.txt", Writes),
            ("cat <> y", Writes),
            ("> y", Writes),
            ("ls '>' y", ReadOnly),
            ("echo x | tee y", Writes),
            ("echo x | tee", Writes),
            ("sort -o out in", Writes),
            ("sort -rof out in", Writes),
            ("sort --output=out in", Writes),
            ("sort --out=out in", Writes),
            ("sort --compress-program=sh in", Unknown),
            ("uniq in out", Writes),
            ("xxd in out", Writes),
            ("xxd -r dump bin", Writes),
            ("file -C -m magic", Writes),
            ("file --compile", Writes),
            ("find . -fprint out", Writes),
            ("find . -fls out", Writes),
            ("date -s 2020-01-01", Writes),
            ("date --set=now", Writes),
            ("date 010100002020", Writes),
            ("cp a b", Writes),
            ("mv a b", Writes),
            ("touch a", Writes),
            ("mkdir -p a/b", Writes),
            ("chmod +x run.sh", Writes),
            ("ln -s a b", Writes),
            ("dd if=/dev/zero of=disk", Writes),
            ("ls | xargs touch", Writes),
        ],
    );
}

#[test]
fn substitutions_and_shell_syntax_are_not_read_only() {
    check(
        Dialect::Sh,
        &[
            ("ls $(whoami)", Unknown),
            ("echo \"$(rm x)\"", Unknown),
            ("echo `rm x`", Unknown),
            ("echo \"`rm x`\"", Unknown),
            ("$(echo rm) x", Unknown),
            ("ls ${HOME}", Unknown),
            ("ls $HOME", Unknown),
            ("ls \"$HOME\"", Unknown),
            ("ls $1", Unknown),
            ("$cmd x", Unknown),
            ("echo $((1 + 2))", Unknown),
            ("echo $[1 + 2]", Unknown),
            ("$'\\x72m' x", Unknown),
            ("echo $\"x\"", Unknown),
            ("diff <(ls a) <(ls b)", Unknown),
            ("ls >(cat)", Unknown),
            ("ls &", Unknown),
            ("(ls)", Unknown),
            ("{ ls; }", Unknown),
            ("cat <<EOF\nx\nEOF", Unknown),
            ("cat <<< x", Unknown),
            ("ls # ; rm x", Unknown),
            ("X=1 ls", Unknown),
            ("X+=1 ls", Unknown),
            ("PATH=/tmp ls", Unknown),
            ("X=1", Unknown),
            ("env X=1 ls", Unknown),
            ("env ls", Unknown),
            ("eval ls", Unknown),
            ("exec ls", Unknown),
            ("source ./x", Unknown),
            (". ./x", Unknown),
            ("ls | sh", Unknown),
            ("ls | bash -s", Unknown),
            ("sh -c ls", Unknown),
            ("ls | xargs sh -c 'rm x'", Unknown),
            ("ls | xargs sort", Unknown),
            ("ls | xargs -p cat", Unknown),
            ("ls | xargs /bin/cat", Unknown),
            ("find . -exec rm {} \\;", Unknown),
            ("find . -execdir ls \\;", Unknown),
            ("find . -ok rm {} +", Unknown),
            ("find . -'exec' rm {} +", Unknown),
            ("find *", Unknown),
            ("sort *", Unknown),
            ("rg --pre sh x", Unknown),
            ("rg --pre=./x pattern", Unknown),
            ("rg x *", Unknown),
            ("printf -v PATH /tmp", Unknown),
            ("date --bogus", Unknown),
            ("date -x", Unknown),
            ("ls ;; pwd", Unknown),
            ("ls;", Unknown),
            ("; ls", Unknown),
            ("ls &&", Unknown),
            ("ls |", Unknown),
            ("ls >", Unknown),
            ("ls <&x", Unknown),
            ("echo 'unterminated", Unknown),
            ("echo \"unterminated", Unknown),
            ("echo x\\", Unknown),
            ("{rm,-rf,x}", Unknown),
            ("find . {-delete,-print}", Unknown),
            ("ls {a,b}", ReadOnly),
            ("l* -a", Unknown),
            ("time ls", Unknown),
            ("command ls", Unknown),
            ("nohup ls", Unknown),
            ("! ls", Unknown),
            ("python3 -c 'print(1)'", Unknown),
            ("node -e 1", Unknown),
            ("ruby -e 1", Unknown),
            ("sed -n 1p file", Unknown),
            ("awk '{print}' file", Unknown),
            ("curl -o x https://example.com", Unknown),
            ("wget https://example.com", Unknown),
            ("frobnicate", Unknown),
        ],
    );
}

#[test]
fn quoting_and_lookalike_program_names_never_read_as_safe() {
    check(
        Dialect::Sh,
        &[
            ("'rm' x", Deletes),
            ("\"rm\" x", Deletes),
            ("r\\m x", Deletes),
            ("\\rm x", Deletes),
            ("r''m x", Deletes),
            ("'ls'", Unknown),
            ("\\ls", Unknown),
            ("l\"s\"", Unknown),
            ("/bin/ls", Unknown),
            ("./ls", Unknown),
            ("/bin/rm -rf x", Deletes),
            ("/usr/bin/sudo ls", Elevates),
            ("\u{217c}s", Unknown),
            ("ｌｓ", Unknown),
            ("l\u{200b}s", Unknown),
            ("ls\u{a0}-la", Unknown),
            ("LS", Unknown),
            ("ls\u{7}", Unknown),
            ("ls\r\nrm x", Unknown),
            ("ls\u{1b}[2J", Unknown),
        ],
    );
}

#[test]
fn lists_take_the_strongest_part() {
    check(
        Dialect::Sh,
        &[
            ("ls; rm x", Deletes),
            ("ls && rm x", Deletes),
            ("ls || rm x", Deletes),
            ("ls\nrm x", Deletes),
            ("ls | rm x", Deletes),
            ("rm x; frobnicate", Deletes),
            ("frobnicate; cp a b", Writes),
            ("cp a b; rm c", Deletes),
            ("rm c; apt install x", Installs),
            ("apt install x; sudo ls", Elevates),
            ("sudo ls; rm x", Elevates),
            ("cat x > y; ls", Writes),
        ],
    );
}

#[test]
fn deletions_installs_and_elevation_are_named() {
    check(
        Dialect::Sh,
        &[
            ("rm -rf target", Deletes),
            ("rmdir empty", Deletes),
            ("unlink a", Deletes),
            ("shred -u secret", Deletes),
            ("find . -name '*.tmp' -delete", Deletes),
            ("find . -delete -exec ls \\;", Deletes),
            ("rm $FILE", Deletes),
            ("X=1 rm x", Deletes),
            ("apt install ripgrep", Installs),
            ("apt-get update", Installs),
            ("dnf install x", Installs),
            ("yum remove x", Installs),
            ("pacman -S x", Installs),
            ("zypper in x", Installs),
            ("brew install x", Installs),
            ("pip install requests", Installs),
            ("pip3 install --user x", Installs),
            ("pip list", Unknown),
            ("cargo install ripgrep", Installs),
            ("cargo build", Unknown),
            ("npm install -g x", Installs),
            ("npm i --global x", Installs),
            ("yarn global add x", Installs),
            ("npm install", Unknown),
            ("gem install x", Installs),
            ("go install x@latest", Installs),
            ("sudo ls", Elevates),
            ("sudo rm -rf /", Elevates),
            ("sudo $(x)", Unknown),
            ("doas ls", Elevates),
            ("su -c ls", Elevates),
            ("pkexec ls", Elevates),
            ("ls | xargs sudo rm", Elevates),
            ("ls | xargs rm", Deletes),
        ],
    );
}

#[test]
fn empty_and_oversized_commands_are_unknown() {
    check(Dialect::Sh, &[("", Unknown), ("   ", Unknown), ("\n\n", Unknown), ("\t", Unknown)]);

    let long = "ls ".repeat(3000);

    assert_eq!(classify(&long, Dialect::Sh).kind, Unknown);

    let fits = format!("ls {}", "a".repeat(4000));

    assert_eq!(classify(&fits, Dialect::Sh).kind, ReadOnly);

    let piped = vec!["cat a"; 500].join(" | ");

    assert_eq!(classify(&piped, Dialect::Sh).kind, ReadOnly);

    let nested = format!("ls | {}", vec!["xargs"; 1000].join(" "));

    assert_eq!(classify(&nested, Dialect::Sh).kind, Unknown);
}

#[test]
fn verdicts_explain_themselves() {
    let verdict = classify("ls -la", Dialect::Sh);

    assert!(verdict.is_read_only());
    assert_eq!(verdict.reason, "`ls` only reads");
    assert_eq!(classify("rm x", Dialect::Sh).reason, "`rm` deletes files");
    assert_eq!(classify("sudo ls", Dialect::Sh).reason, "`sudo` runs with admin rights");
    assert_eq!(classify("ls $(x)", Dialect::Sh).reason, "the command uses command substitution");

    assert_eq!(
        classify("cat a > b", Dialect::Sh).reason,
        "the command redirects output into a file"
    );

    assert!(!classify("frobnicate", Dialect::Sh).is_read_only());
}

#[test]
fn cmd_commands_are_classified() {
    check(
        Dialect::Cmd,
        &[
            ("dir", ReadOnly),
            ("dir /s /b C:\\src", ReadOnly),
            ("DIR", ReadOnly),
            ("type README.md", ReadOnly),
            ("findstr /n main src\\main.rs", ReadOnly),
            ("where cargo", ReadOnly),
            ("ver", ReadOnly),
            ("echo hello", ReadOnly),
            ("type a.txt | findstr x", ReadOnly),
            ("dir && ver", ReadOnly),
            ("dir & ver", ReadOnly),
            ("dir || ver", ReadOnly),
            ("dir 2>nul", ReadOnly),
            ("dir >NUL 2>&1", ReadOnly),
            ("type \"my file.txt\"", ReadOnly),
            ("type < a.txt", ReadOnly),
            ("git status", ReadOnly),
            ("dir > out.txt", Writes),
            ("dir>>out.txt", Writes),
            ("dir 2> err.txt", Writes),
            ("copy a b", Writes),
            ("move a b", Writes),
            ("mkdir x", Writes),
            ("dir & del x", Deletes),
            ("del /q x", Deletes),
            ("erase x", Deletes),
            ("rd /s /q x", Deletes),
            ("rmdir x", Deletes),
            ("del,x", Deletes),
            ("C:\\Windows\\System32\\where.exe x", Unknown),
            ("where.exe x", Unknown),
            ("dir/s", Unknown),
            ("\"dir\"", Unknown),
            ("winget install x", Installs),
            ("choco install x", Installs),
            ("msiexec /i x.msi", Installs),
            ("runas /user:admin cmd", Elevates),
            ("echo %PATH%", Unknown),
            ("echo !x!", Unknown),
            ("d^el x", Unknown),
            ("(dir)", Unknown),
            ("dir\ndel x", Unknown),
            ("dir \"x", Unknown),
            ("dir >&x", Unknown),
            ("cmd /c del x", Unknown),
            ("powershell -c ls", Unknown),
            ("start x", Unknown),
            ("set X=1", Unknown),
            ("type \"a & del b\"", ReadOnly),
        ],
    );
}

#[test]
fn powershell_commands_are_classified() {
    check(
        Dialect::PowerShell,
        &[
            ("Get-ChildItem", ReadOnly),
            ("get-childitem -Recurse -Filter *.rs", ReadOnly),
            ("ls", ReadOnly),
            ("dir C:\\src", ReadOnly),
            ("gci -Force", ReadOnly),
            ("Get-Content README.md -TotalCount 10", ReadOnly),
            ("cat a.txt", ReadOnly),
            ("type a.txt", ReadOnly),
            ("Select-String -Path *.rs -Pattern main", ReadOnly),
            ("Test-Path 'C:\\Program Files'", ReadOnly),
            ("Get-ChildItem -Include *.rs", ReadOnly),
            ("Get-Content a.txt | Select-String x | Sort-Object", ReadOnly),
            ("Get-Location; Get-Date", ReadOnly),
            ("Get-ChildItem && Get-Location", ReadOnly),
            ("ls 2>$null", ReadOnly),
            ("ls *> $null", ReadOnly),
            ("ls 2>&1 | Select-Object -First 3", ReadOnly),
            ("Write-Output 'it''s'", ReadOnly),
            ("echo \"say \"\"hi\"\"\"", ReadOnly),
            ("git log --author=me@example.com", ReadOnly),
            ("findstr x a.txt", ReadOnly),
            ("ls > out.txt", Writes),
            ("ls >> out.txt", Writes),
            ("ls 2> err.txt", Writes),
            ("ls | Out-File out.txt", Writes),
            ("ls | Set-Content out.txt", Writes),
            ("ls | Tee-Object out.txt", Writes),
            ("New-Item x", Writes),
            ("Copy-Item a b", Writes),
            ("Remove-Item x", Deletes),
            ("rm -Recurse -Force x", Deletes),
            ("del x", Deletes),
            ("ri x", Deletes),
            ("Microsoft.PowerShell.Management\\Remove-Item x", Deletes),
            ("ls; Remove-Item x", Deletes),
            ("'rm' x", Deletes),
            ("Install-Module x", Installs),
            ("Install-Package x", Installs),
            ("winget install x", Installs),
            ("Start-Process pwsh -Verb RunAs", Elevates),
            ("Start-Process notepad", Unknown),
            ("sudo ls", Elevates),
            ("Get-Content a -OutVariable x", Unknown),
            ("ls -ov x", Unknown),
            ("ls -PipelineVariable:x", Unknown),
            ("ls $env:HOME", Unknown),
            ("ls \"$env:HOME\"", Unknown),
            ("ls $(rm x)", Unknown),
            ("ls (rm x)", Unknown),
            ("ls @args", Unknown),
            ("ls {rm x}", Unknown),
            ("& rm x", Unknown),
            ("& 'ls'", Unknown),
            ("ls `; rm x", Unknown),
            ("ls # x", Unknown),
            ("ls < x", Unknown),
            ("ls --% x", Unknown),
            ("ls \u{2013}Force", Unknown),
            ("ls \u{2018}x\u{2019}", Unknown),
            ("ls 'x", Unknown),
            ("ls \"x", Unknown),
            ("ls >&x", Unknown),
            ("iex 'rm x'", Unknown),
            ("Invoke-Expression x", Unknown),
            ("Invoke-WebRequest x", Unknown),
            ("where x", Unknown),
            ("[IO.File]::Delete('x')", Unknown),
            ("$x = 1", Unknown),
            ("Get-Process", Unknown),
        ],
    );
}
