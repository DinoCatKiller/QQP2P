const { execSync } = require('child_process');
const fs = require('fs');

console.log('=== Checking QQ processes and NapCat DLL ===');

// Get all QQ PIDs
try {
    const tasklist = execSync('tasklist /FI "IMAGENAME eq QQ.exe" /FO CSV /NH', { encoding: 'utf8' });
    const lines = tasklist.trim().split('\n');
    const qqPids = [];
    lines.forEach(line => {
        const m = line.match(/"(\d+)"/);
        if (m) qqPids.push(m[1]);
    });
    console.log('QQ PIDs:', qqPids.join(', '));
} catch (e) {
    console.log('Failed to get QQ PIDs');
}

// Check for NapCat DLLs in QQ processes
console.log('\n=== Checking for NapCat DLL in QQ processes ===');
const dllCheck = execSync('tasklist /FI "IMAGENAME eq QQ.exe" /M NapCat*', { encoding: 'utf8', shell: 'cmd.exe' });
console.log(dllCheck || 'No NapCat DLL found in any QQ process');

// Check named pipes
console.log('\n=== Named Pipes ===');
try {
    const pipes = execSync('dir \\\\.\\pipe\\NapCat_* 2>nul', { encoding: 'utf8', shell: 'cmd.exe' });
    console.log(pipes || 'No NapCat pipes found');
} catch (e) {
    console.log('No NapCat pipes');
}

// Check for QQ logs
console.log('\n=== QQ Logs ===');
try {
    const logPaths = [
        'C:/Users/28643/Documents/QQ/2864305010/Log',
        'C:/Users/28643/Documents/Tencent/QQ/2864305010/Log',
        'C:/Users/28643/Documents/QQNT/2864305010/Log'
    ];
    logPaths.forEach(p => {
        if (fs.existsSync(p)) {
            const files = fs.readdirSync(p).sort().reverse().slice(0, 3);
            console.log(p + ':', files.join(', '));
        }
    });
} catch (e) {
    console.log('No QQ logs found');
}

// Check NapCat logs
console.log('\n=== NapCat Logs ===');
const napcatLogPath = 'H:/NapCat/logs';
if (fs.existsSync(napcatLogPath)) {
    const files = fs.readdirSync(napcatLogPath).sort().reverse().slice(0, 3);
    console.log('NapCat logs:', files.join(', ') || 'empty');
    files.forEach(f => {
        try {
            const c = fs.readFileSync(napcatLogPath + '/' + f, 'utf8');
            console.log('--- ' + f + ' ---');
            console.log(c.slice(-500));
        } catch (e) {}
    });
} else {
    console.log('No NapCat logs directory');
}

console.log('\n=== Done ===');
