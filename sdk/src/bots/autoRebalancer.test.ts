import { ArbitrageScanner, ArbitrageExecutor, Opportunity } from '../arbitrage';
import { WriteRateLimiter } from '../utils/rateLimiter';
import { AutoRebalancer, RebalancerConfig } from './autoRebalancer';

function opportunity(overrides: Partial<Opportunity> = {}): Opportunity {
  return {
    pool_id: 'POOL-B',
    current_apy: 1500,
    projected_apy: 2000,
    il_risk: 50,
    net_profit: BigInt(1000),
    apy_delta: 500,
    recommended: true,
    ...overrides,
  };
}

function config(): RebalancerConfig {
  return {
    vaultAddress: 'VAULT',
    currentPoolId: 'POOL-A',
    currentVaultApy: 1500,
    minApyDelta: 200,
    maxIlTolerance: 100,
    cooldownHours: 0,
    riskTolerance: 5,
    maxPositionSize: BigInt(1_000_000_000),
  };
}

/** Scanner stub that always reports a single profitable opportunity. */
function scannerStub(): ArbitrageScanner {
  return {
    findOpportunities: async () => [opportunity()],
  } as unknown as ArbitrageScanner;
}

/** A virtual clock whose time only advances when `sleep` is called. */
function virtualClock() {
  let time = 0;
  return {
    now: () => time,
    sleep: async (ms: number) => {
      time += ms;
    },
  };
}

describe('AutoRebalancer write pacing', () => {
  it('routes keeper writes through the executor write limiter', async () => {
    const limiter = new WriteRateLimiter({ minIntervalMs: 0 });
    const executor = new ArbitrageExecutor('https://horizon-testnet.stellar.org', {
      writeLimiter: limiter,
    });
    const keeper = new AutoRebalancer(config(), { scanner: scannerStub(), executor });

    const results = await keeper.run();

    expect(results).toHaveLength(1);
    expect(results[0].success).toBe(true);
    // The keeper's write went through the limiter.
    expect(limiter.stats().completed).toBe(1);
    expect(keeper.getWriteLimiterStats().completed).toBe(1);
  });

  it('throttles keeper writes when the pacing interval has not elapsed', async () => {
    const clock = virtualClock();
    const limiter = new WriteRateLimiter({
      minIntervalMs: 1000,
      maxConcurrent: 1,
      now: clock.now,
      sleep: clock.sleep,
    });
    const executor = new ArbitrageExecutor('https://horizon-testnet.stellar.org', {
      writeLimiter: limiter,
    });
    const keeper = new AutoRebalancer(config(), { scanner: scannerStub(), executor });

    // A prior write at t=0 consumes the pacing window.
    await limiter.schedule(async () => undefined);
    expect(clock.now()).toBe(0);

    const results = await keeper.run();

    expect(results[0].success).toBe(true);
    // The keeper's write had to wait out the 1000ms pacing interval.
    expect(clock.now()).toBe(1000);
    expect(keeper.getWriteLimiterStats().throttled).toBe(1);
  });
});
