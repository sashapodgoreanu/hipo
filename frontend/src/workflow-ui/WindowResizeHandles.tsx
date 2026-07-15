import { useEffect, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { isTauri } from '../tauri-dialog';

/**
 * Invisible edge/corner grips that let the frameless window be resized
 * (issue #182). With `decorations: false` a Linux/GTK window has no native
 * resize border, so once un-maximized it is stuck at its size. Each grip
 * forwards a mouse-down to Tauri's `startResizeDragging`, which hands the
 * drag to the OS. Rendered only under Tauri and hidden while maximized.
 */
const EDGES = [
    { cls: 'win-resize-n', dir: 'North' },
    { cls: 'win-resize-s', dir: 'South' },
    { cls: 'win-resize-w', dir: 'West' },
    { cls: 'win-resize-e', dir: 'East' },
    { cls: 'win-resize-nw', dir: 'NorthWest' },
    { cls: 'win-resize-ne', dir: 'NorthEast' },
    { cls: 'win-resize-sw', dir: 'SouthWest' },
    { cls: 'win-resize-se', dir: 'SouthEast' },
] as const;

export default function WindowResizeHandles() {
    const [maximized, setMaximized] = useState(false);

    useEffect(() => {
        if (!isTauri()) return;
        const win = getCurrentWindow();
        let unlisten: (() => void) | undefined;
        void win.isMaximized().then(setMaximized).catch(() => {});
        win.onResized(() => {
            void win.isMaximized().then(setMaximized).catch(() => {});
        })
            .then(u => {
                unlisten = u;
            })
            .catch(() => {});
        return () => unlisten?.();
    }, []);

    if (!isTauri() || maximized) return null;

    return (
        <div className="win-resize" aria-hidden="true">
            {EDGES.map(({ cls, dir }) => (
                <div
                    key={cls}
                    className={cls}
                    onMouseDown={e => {
                        if (e.button !== 0) return;
                        e.preventDefault();
                        void getCurrentWindow().startResizeDragging(dir);
                    }}
                />
            ))}
        </div>
    );
}
