import AppKit
import Carbon.HIToolbox

/// Global hotkey (default ⌘⇧Space) that fires `callback` when pressed.
final class HotKey {
    private var ref: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private let callback: () -> Void

    init(keyCode: UInt32 = UInt32(kVK_Space),
         modifiers: UInt32 = UInt32(cmdKey | shiftKey),
         callback: @escaping () -> Void) {
        self.callback = callback

        let selfPtr = Unmanaged.passUnretained(self).toOpaque()
        var spec = EventTypeSpec(
            eventClass: OSType(kEventClassKeyboard),
            eventKind: UInt32(kEventHotKeyPressed)
        )
        InstallEventHandler(GetApplicationEventTarget(), { _, _, userData in
            guard let userData else { return noErr }
            Unmanaged<HotKey>.fromOpaque(userData)
                .takeUnretainedValue()
                .callback()
            return noErr
        }, 1, &spec, selfPtr, &handler)

        // Signature 'LTSU' = LotusGrid, arbitrary but must be stable.
        let id = EventHotKeyID(signature: OSType(0x4C545355), id: 1)
        var hotKeyRef: EventHotKeyRef?
        RegisterEventHotKey(keyCode, modifiers, id, GetApplicationEventTarget(), 0, &hotKeyRef)
        self.ref = hotKeyRef
    }

    deinit {
        if let ref { UnregisterEventHotKey(ref) }
        if let handler { RemoveEventHandler(handler) }
    }
}
