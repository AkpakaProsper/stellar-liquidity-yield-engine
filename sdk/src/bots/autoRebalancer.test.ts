import { AutoRebalancer, RebalancerConfig } from './autoRebalancer';

function baseConfig(overrides: Partial<RebalancerConfig> = {}): RebalancerConfig {
  return {
    vaultAddress: 'VAULT',
    currentPoolId: 'POOL',
    currentVaultApy: 1500,
    minApyDelta: 200,
    maxIlTolerance: 100,
    cooldownHours: 24,
    riskTolerance: 5,
    maxPositionSize: BigInt(1_000_000_000),
    ...overrides,
  };
}

describe('AutoRebalancer RPC lag wiring', () => {
  it('reports no lag when no head source is configured', async () => {
    const keeper = new AutoRebalancer(baseConfig());

    expect(await keeper.checkRpcLag()).toBeUndefined();
    expect(keeper.getLastLagReport()).toBeUndefined();
  });

  it('detects and records a lagging node through the configured head source', async () => {
    const keeper = new AutoRebalancer(
      baseConfig({
        rpcHeadSource: { getLatestLedger: async () => ({ sequence: 2000 }) },
        lagOptions: { warningLedgers: 5, criticalLedgers: 20 },
      }),
    );

    const report = await keeper.checkRpcLag();

    expect(report).toBeDefined();
    expect(report!.lagLedgers).toBe(2000);
    expect(report!.severity).toBe('critical');
    expect(report!.message).toMatch(/critically behind/);
    expect(keeper.getLastLagReport()).toEqual(report);
  });

  it('reports ok when the node head is current', async () => {
    const keeper = new AutoRebalancer(
      baseConfig({ rpcHeadSource: { getLatestLedger: async () => ({ sequence: 0 }) } }),
    );

    const report = await keeper.checkRpcLag();

    expect(report!.severity).toBe('ok');
    expect(report!.lagLedgers).toBe(0);
  });
});

describe('AutoRebalancer cron scheduling', () => {
  it('isReadyForRebalance returns true when cooldown has passed (cooldown-only mode)', () => {
    const config = baseConfig({
      cooldownHours: 1,
      cronExpression: undefined,
    } as RebalancerConfig);

    const keeper = new AutoRebalancer(config);

    ;(keeper as any).lastExecutionTime = Date.now() - 2 * 3600 * 1000;

    expect(keeper['isReadyForRebalance']()).toBe(true);
  });

  it('isReadyForRebalance returns true when cron expression is set and cooldown satisfied (combined mode)', () => {
    const config = baseConfig({
      cooldownHours: 1,
      cronExpression: '*',
    } as RebalancerConfig);

    const keeper = new AutoRebalancer(config);

    ;(keeper as any).lastExecutionTime = Date.now() - 3600000;

    expect(keeper['isReadyForRebalance']()).toBe(true);
  });

  it('isReadyForRebalance returns false when cron matches but cooldown not satisfied', () => {
    const config = baseConfig({
      cooldownHours: 24,
      cronExpression: '*',
    } as RebalancerConfig);

    const keeper = new AutoRebalancer(config);

    ;(keeper as any).lastExecutionTime = Date.now();

    expect(keeper['isReadyForRebalance']()).toBe(false);
  });

  it('isReadyForRebalance returns false when cooldown satisfied outside cron schedule', () => {
    const config = baseConfig({
      cooldownHours: 1,
      cronExpression: '99', // minute 99 never matches
    } as RebalancerConfig);

    const keeper = new AutoRebalancer(config);

    ;(keeper as any).lastExecutionTime = Date.now() - 3600000;

    expect(keeper['isReadyForRebalance']()).toBe(false);
  });

  it('prevents repeated rebalances during the same matching minute', () => {
    const config = baseConfig({
      cooldownHours: 1,
      cronExpression: '*',
    } as RebalancerConfig);

    const keeper = new AutoRebalancer(config);

    ;(keeper as any).lastExecutionTime = Date.now() - 3600000;

    const firstResult = keeper['isReadyForRebalance']();
    expect(firstResult).toBe(true);

    const secondResult = keeper['isReadyForRebalance']();
    expect(secondResult).toBe(false);
  });

  it('a later matching minute is eligible again after consumption', () => {
    const config = baseConfig({
      cooldownHours: 1,
      cronExpression: '*',
    } as RebalancerConfig);

    const keeper = new AutoRebalancer(config);

    ;(keeper as any).lastExecutionTime = Date.now() - 3600000;

    // First call: cron '*' always matches, cooldown satisfied, consumes current minute
    const firstResult = keeper['isReadyForRebalance']();
    expect(firstResult).toBe(true);

    // Same minute: already consumed, returns false
    const secondResult = keeper['isReadyForRebalance']();
    expect(secondResult).toBe(false);

    // Later minute: new minute, eligible again
    ;(keeper as any).lastConsumedMinute = -1;

    const now = new Date();
    now.setMinutes((now.getMinutes() + 15) % 60);

    ;(keeper as any).lastExecutionTime = Date.now() - 3600000;

    const result = keeper['isReadyForRebalance']();
    expect(result).toBe(true);
  });
});