import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { clampConcurrentQueries, positiveNumber, SettingsModal } from './SettingsModal';

const bridge = vi.hoisted(() => ({
    settingsGetProxy: vi.fn(),
    settingsSetProxy: vi.fn(),
    settingsGetAi: vi.fn(),
    settingsSetAi: vi.fn(),
    settingsGetRunnerResources: vi.fn(),
    settingsSetRunnerResources: vi.fn(),
    settingsGetAllowUnsigned: vi.fn(),
    settingsSetAllowUnsigned: vi.fn(),
    settingsGetContextFile: vi.fn(),
    settingsSetContextFile: vi.fn(),
}));

vi.mock('../tauri-bridge', () => bridge);

const profile = {
    version: 3,
    memory: { mode: 'automatic' as const },
    cpuThreads: { mode: 'automatic' as const },
    spill: { mode: 'automatic' as const },
    quackParallelism: { mode: 'automatic' as const },
    baseCapacity: 3,
};

describe('SettingsModal runner resources', () => {
    beforeEach(() => {
        localStorage.clear();
        localStorage.setItem('duckle:v1:settingsExpanded', JSON.stringify(['runner-resources']));
        vi.clearAllMocks();
        bridge.settingsGetProxy.mockResolvedValue(null);
        bridge.settingsGetAi.mockResolvedValue({ baseUrl: null, model: null, apiKey: null });
        bridge.settingsGetRunnerResources.mockResolvedValue({
            requested: profile,
            effective: profile,
            diagnostics: ['host_limit'],
        });
        bridge.settingsGetContextFile.mockResolvedValue(null);
        bridge.settingsGetAllowUnsigned.mockResolvedValue(false);
        bridge.settingsSetProxy.mockResolvedValue(undefined);
        bridge.settingsSetAi.mockResolvedValue(undefined);
        bridge.settingsSetRunnerResources.mockImplementation(async (_workspace, nextProfile) => ({
            requested: nextProfile,
            effective: nextProfile,
            diagnostics: [],
        }));
        bridge.settingsSetAllowUnsigned.mockResolvedValue(undefined);
        bridge.settingsSetContextFile.mockResolvedValue(undefined);
    });

    it('clamps manual resource inputs to their supported ranges', () => {
        expect(clampConcurrentQueries('0')).toBe(1);
        expect(clampConcurrentQueries('4')).toBe(4);
        expect(clampConcurrentQueries('9')).toBe(8);
        expect(positiveNumber('0', 1)).toBe(1);
        expect(positiveNumber('5', 1)).toBe(5);
    });

    it('loads diagnostics and saves a manual concurrent-query profile', async () => {
        const user = userEvent.setup();
        const workspace = 'C:\\workspace';
        render(<SettingsModal workspace={workspace} onClose={vi.fn()} />);

        expect(await screen.findByText(/effective profile constrained by: host limit/i)).toBeTruthy();

        await user.selectOptions(screen.getByLabelText('Concurrent query mode'), 'value');

        await user.click(screen.getByRole('button', { name: 'Save' }));
        await waitFor(() => {
            expect(bridge.settingsSetRunnerResources).toHaveBeenCalledWith(
                workspace,
                expect.objectContaining({
                    quackParallelism: { mode: 'value', value: 1 },
                }),
            );
        });
    });
});
