const { execSync } = require('child_process');
const fs = require('fs');

// Check if NapCat DLL is loaded in any QQ process
console.log('=== Checking QQ processes ===');
try {
    const result = execSync('tasklist /FI "IMAGENAME eq QQ.exe" /V', { encoding: 'utf8' });
    console.log(result.slice(0, 1000));
} catch (e) {
    console.log('tasklist error:', e.message);
}

// Check named pipes for NapCat
console.log('\n=== Checking NapCat pipes ===');
try {
    const result = execSync('dir \\\\.\\pipe\\NapCat_* 2>nul', { encoding: 'utf8' });
    console.log(result || 'no NapCat pipes found');
} catch (e) {
    console.log('pipe check:', e.message);
}

// Check listening ports
console.log('\n=== Checking ports ===');
try {
    const result = execSync('netstat -ano | findstr LISTENING', { encoding: 'utf8' });
    const lines = result.split('\n').filter(l => l.includes(':3000') || l.includes(':3001') || l.includes(':7778'));
    console.log(lines.join('\n') || 'no NapCat ports');
} catch (e) {
    console.log('port check:', e.message);
}

// Try HTTP API
console.log('\n=== Testing HTTP API ===');
const http = require('http');
const tests = [3000, 3001, 7778];
for (const port of tests) {
    const opts = { hostname: '127.0.0.1', port, path: '/get_login_info', timeout: 2000 };
    http.get(opts, res => {
        let d = '';
        res.on('data', c => d += c);
        res.on('end', () => console.log(`Port ${port}: ${res.statusCode}`, d.slice(0, 100)));
    }).on('error', e => console.log(`Port ${port}: ${e.code}`));
}

setTimeout(() => process.exit(0), 5000);
