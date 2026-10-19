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
    expect(report!.lagLedgers).toBe(2000); // processed marker still 0
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
