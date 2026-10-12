import Foundation
import CFNetwork

// Reads OS network policy only. No Apple Events, GUI, settings writes, or PAC execution.
enum ProxyProbeError: Error { case invalidInput, unavailable }

do {
    let input = FileHandle.standardInput.readData(ofLength: 4097)
    guard input.count <= 4096,
          let value = String(data: input, encoding: .utf8),
          let components = URLComponents(string: value.trimmingCharacters(in: .whitespacesAndNewlines)),
          ["http", "https"].contains(components.scheme?.lowercased() ?? ""),
          components.user == nil, components.password == nil,
          components.host?.isEmpty == false,
          let url = components.url,
          let settings = CFNetworkCopySystemProxySettings()?.takeRetainedValue() else {
        throw ProxyProbeError.invalidInput
    }
    let proxies = CFNetworkCopyProxiesForURL(url as CFURL, settings).takeRetainedValue()
    guard let entries = proxies as? [[String: Any]], entries.count <= 16 else {
        throw ProxyProbeError.unavailable
    }
    var result: [[String: Any]] = []
    for entry in entries {
        guard let type = entry[kCFProxyTypeKey as String] as? String else {
            throw ProxyProbeError.unavailable
        }
        var item: [String: Any] = [
            "type": type,
            "hasAuthentication": entry[kCFProxyUsernameKey as String] != nil
                || entry[kCFProxyPasswordKey as String] != nil,
        ]
        if let host = entry[kCFProxyHostNameKey as String] as? String {
            guard !host.isEmpty, host.utf8.count <= 255,
                  host.rangeOfCharacter(from: .whitespacesAndNewlines) == nil,
                  !host.contains("@"), !host.contains("/"), !host.contains("\0") else {
                throw ProxyProbeError.invalidInput
            }
            item["host"] = host
        }
        if let port = entry[kCFProxyPortNumberKey as String] as? NSNumber { item["port"] = port.intValue }
        // Deliberately omit proxy credentials and PAC URL/script contents.
        result.append(item)
    }
    let data = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
} catch {
    FileHandle.standardError.write(Data("system_proxy_resolution_failed\n".utf8))
    exit(2)
}
