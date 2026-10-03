import Foundation

/// Word timing for the transcript highlight, ported from the React Native
/// `karaoke.ts`.
///
/// The displayed text and the ASR word list are not always the same tokens —
/// a line may have been edited, or punctuation attached differently — so each
/// displayed token is anchored to an ASR word when the two match within a short
/// look-ahead, and tokens in between are spread across the gap by length.
enum Karaoke {
    struct Token: Hashable {
        let text: String
        let startMs: Int
        let endMs: Int
    }

    struct Line {
        let id: Int
        let startMs: Int
        let endMs: Int
        let tokens: [Token]
    }

    /// How long a word stays lit after it ends, so a quick word does not
    /// flicker off before the eye has found it.
    static let lingerMs = 120
    private static let lookahead = 4

    static func timeline(_ utterances: [Utterance]) -> [Line] {
        utterances.map { Line(id: $0.id, startMs: $0.startMs, endMs: $0.endMs, tokens: tokens(for: $0)) }
            .sorted { $0.startMs < $1.startMs }
    }

    static func tokens(for u: Utterance) -> [Token] {
        let texts = u.text.split(whereSeparator: \.isWhitespace).map(String.init)
        guard !texts.isEmpty else { return [] }
        let timed = u.words.filter { $0.endMs > $0.startMs }
        if timed.isEmpty {
            return u.endMs > u.startMs ? interpolate(texts, u.startMs, u.endMs) : []
        }

        var anchors = [Word?](repeating: nil, count: texts.count)
        var next = 0
        for i in texts.indices where next < timed.count {
            let key = normalize(texts[i])
            for k in next..<min(timed.count, next + lookahead) where normalize(timed[k].w) == key {
                anchors[i] = timed[k]
                next = k + 1
                break
            }
        }

        var out = [Token]()
        out.reserveCapacity(texts.count)
        var i = 0
        var prevEnd = min(u.startMs, timed[0].startMs)
        while i < texts.count {
            if let a = anchors[i] {
                out.append(Token(text: texts[i], startMs: a.startMs, endMs: a.endMs))
                prevEnd = a.endMs
                i += 1
                continue
            }
            var j = i
            while j < texts.count && anchors[j] == nil { j += 1 }
            let nextStart = j < texts.count ? anchors[j]!.startMs : max(prevEnd, u.endMs)
            out.append(contentsOf: interpolate(Array(texts[i..<j]), prevEnd, max(prevEnd, nextStart)))
            prevEnd = nextStart
            i = j
        }
        return out
    }

    /// Index of the last item starting at or before `ms`, or -1.
    static func lastStarted<T>(_ items: [T], before ms: Int, start: (T) -> Int) -> Int {
        var lo = 0, hi = items.count - 1, found = -1
        while lo <= hi {
            let mid = (lo + hi) / 2
            if start(items[mid]) <= ms { found = mid; lo = mid + 1 } else { hi = mid - 1 }
        }
        return found
    }

    /// The word being spoken at `ms`, or nil in a pause.
    static func activeToken(_ tokens: [Token], at ms: Int) -> Int? {
        let i = lastStarted(tokens, before: ms) { $0.startMs }
        guard i >= 0, ms < tokens[i].endMs + lingerMs else { return nil }
        return i
    }

    private static func normalize(_ s: String) -> String {
        let lower = s.lowercased()
        let stripped = lower.filter { $0.isLetter || $0.isNumber || $0 == "'" }
        return stripped.isEmpty ? lower : stripped
    }

    private static func interpolate(_ texts: [String], _ start: Int, _ end: Int) -> [Token] {
        let span = Double(max(0, end - start))
        let weight = Double(max(1, texts.reduce(0) { $0 + $1.count }))
        var cursor = Double(start)
        return texts.map { t in
            let s = cursor
            cursor += span * Double(max(1, t.count)) / weight
            return Token(text: t, startMs: Int(s.rounded()), endMs: Int(cursor.rounded()))
        }
    }
}
