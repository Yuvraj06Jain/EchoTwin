// The widget runs every command through /bin/sh: check that hostile file names stay names.
// Run: node packaging/plasmoid/tests/shell.test.js
const fs = require('fs'), { execFileSync } = require('child_process');
const src = fs.readFileSync(require('path').join(__dirname, '..', 'dev.phonelink.widget', 'contents', 'ui', 'shell.js'), 'utf8').replace('.pragma library', '');
const S = new Function(src + '; return { quote, command, localPath, percent };')();
let ok = 0, bad = 0;
const check = (c, m) => { if (c) ok++; else { bad++; console.log('FAIL', m); } };
const dir = fs.mkdtempSync(require('path').join(require('os').tmpdir(), 'phonelink-quote-'));
for (const name of ["a b.txt", "it's.pdf", "$(touch PWNED).txt", "`touch PWNED`", "x; touch PWNED", "\"q\" \\ & | > <", "new\nline", "ünï 文件"]) {
  const out = execFileSync('/bin/sh', ['-c', 'printf %s ' + S.quote(name)], { cwd: dir }).toString();
  check(out === name, 'round-trip ' + JSON.stringify(name) + ' -> ' + JSON.stringify(out));
}
check(!fs.existsSync(dir + '/PWNED'), 'nothing executed');
// The whole command: a fake phonelink records its arguments.
const fake = dir + '/bin'; fs.mkdirSync(fake);
fs.writeFileSync(fake + '/phonelink', '#!/bin/sh\nfor a in "$@"; do printf "[%s]" "$a"; done\n', { mode: 0o755 });
const out = execFileSync('/bin/sh', ['-c', S.command(['send-file', '--notify', "/tmp/it's $(x).pdf"]) + ' #7'], { env: { PATH: fake + ':/usr/bin:/bin', HOME: dir } }).toString();
check(out === "[send-file][--notify][/tmp/it's $(x).pdf]", 'full command args: ' + out);
check(S.localPath('file:///home/me/My%20File%20%231.pdf') === '/home/me/My File #1.pdf', 'file URL decoded');
check(S.localPath('https://example.com/x') === null, 'web URLs refused');
check(S.localPath('file:///bad%E0%A4%A') === null, 'broken escapes refused');
check(S.percent(5, 0) === 100 && S.percent(50, 200) === 25, 'percent');
console.log(`${ok} passed, ${bad} failed`);
fs.rmSync(dir, { recursive: true, force: true });
process.exit(bad ? 1 : 0);
