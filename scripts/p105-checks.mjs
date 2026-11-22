// Offline suite and independent evidence root for P1-05; never loads cloud credentials.
import { runFoundation } from './p101-checks.mjs';
await runFoundation('artifacts/p105');
