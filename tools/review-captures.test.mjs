import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

test('review capture drivers remain isolated and reproducible', () => {
  execFileSync('python3', ['-B', fileURLToPath(new URL('./review-captures/test_relocation.py', import.meta.url))], { stdio: 'pipe' });
});
