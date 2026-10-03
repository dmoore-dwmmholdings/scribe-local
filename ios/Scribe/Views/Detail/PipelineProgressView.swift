import SwiftUI

/// The server's six-stage checklist for a recording still being processed.
struct PipelineProgressView: View {
    let progress: PipelineProgress

    private static let labels: [String: String] = [
        "transcode": "Preparing audio",
        "diarize": "Identifying speakers",
        "transcribe": "Transcribing speech",
        "merge": "Matching words to speakers",
        "embed": "Building the search index",
        "summarize": "Writing the summary",
    ]

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Text("Processing").font(.subheadline.weight(.semibold)).foregroundStyle(Theme.textPrimary)
                    Spacer()
                    Text("\(progress.completed) of \(progress.total)").font(.caption).foregroundStyle(Theme.textMuted)
                }
                ForEach(progress.stages, id: \.kind) { stage in
                    HStack(spacing: 10) {
                        icon(stage.state).frame(width: 18)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(Self.labels[stage.kind] ?? stage.kind)
                                .font(.footnote)
                                .foregroundStyle(stage.state == .pending ? Theme.textDim : Theme.textPrimary)
                            if let e = stage.error, stage.state == .failed {
                                Text(e).font(.caption2).foregroundStyle(Theme.accentDeep).lineLimit(3)
                            }
                        }
                        Spacer()
                        if stage.state == .running, let started = stage.startedAt.flatMap(ISO8601.parse) {
                            Text(elapsed(since: started, now: context.date))
                                .font(.caption.monospacedDigit()).foregroundStyle(Theme.textMuted)
                        }
                    }
                }
            }
            .padding(14)
            .background(Theme.surface, in: RoundedRectangle(cornerRadius: 14))
        }
    }

    @ViewBuilder private func icon(_ state: StageState) -> some View {
        switch state {
        case .done: Image(systemName: "checkmark.circle.fill").foregroundStyle(Theme.ready)
        case .running: ProgressView().controlSize(.small).tint(Theme.accent)
        case .failed: Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(Theme.accentDeep)
        case .queued: Image(systemName: "clock").foregroundStyle(Theme.textMuted)
        default: Image(systemName: "circle").foregroundStyle(Theme.textDim)
        }
    }

    private func elapsed(since: Date, now: Date) -> String {
        formatClock(ms: Int(now.timeIntervalSince(since) * 1000))
    }
}
