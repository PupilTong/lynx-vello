import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import type { Readable } from 'node:stream';

const subcommand = process.argv[2];
if (subcommand !== 'dev' && subcommand !== 'preview') {
  process.stderr.write(`Usage: node scripts/serve.ts <dev|preview>\n`);
  process.exitCode = 1;
  throw new Error(`unknown subcommand: ${subcommand ?? '(none)'}`);
}

const rspeedy = process.platform === 'win32' ? 'rspeedy.cmd' : 'rspeedy';

const producer = spawn(
  rspeedy,
  [subcommand, '--config', 'lynx.config.producer.ts'],
  { stdio: ['ignore', 'pipe', 'pipe'] },
);

const prefix = (stream: Readable, label: string) => {
  const rl = createInterface({ input: stream });
  rl.on('line', (line) => {
    process.stdout.write(`[${label}] ${line}\n`);
  });
};
prefix(producer.stdout, 'producer');
prefix(producer.stderr, 'producer');

const consumer = spawn(
  rspeedy,
  [subcommand, '--config', 'lynx.config.consumer.ts'],
  { stdio: 'inherit' },
);

const shutdown = (code: number) => {
  if (typeof code === 'number') process.exitCode = code;
  if (!producer.killed) producer.kill('SIGTERM');
  if (!consumer.killed) consumer.kill('SIGTERM');
};

consumer.on('exit', (code) => shutdown(code ?? 0));
producer.on('exit', (code) => shutdown(code ?? 0));
process.on('SIGINT', () => shutdown(0));
process.on('SIGTERM', () => shutdown(0));
