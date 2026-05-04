import Foundation

struct Persistence {
    let url: URL

    init() {
        let appSupport = FileManager.default.urls(
            for: .applicationSupportDirectory, in: .userDomainMask
        )[0]
        let dir = appSupport.appendingPathComponent("LotusGrid")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        self.url = dir.appendingPathComponent("cells.json")
    }

    func load() -> [String: String] {
        guard let data = try? Data(contentsOf: url),
              let dict = try? JSONDecoder().decode([String: String].self, from: data)
        else { return [:] }
        return dict
    }

    func save(_ cells: [String: String]) {
        guard let data = try? JSONEncoder().encode(cells) else { return }
        try? data.write(to: url, options: .atomic)
    }
}
