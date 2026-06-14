/**
 * Screen: Backend Update
 *
 * Admin-only screen that lets an operator upload a signed .tar.gz package
 * to replace the running backend binary in-place, monitor the restart, and
 * roll back to the previous version if needed.
 *
 * Reachable from Settings → "Backend update…".
 * Requires the "Update token" configured in Settings (stored in SecureStore).
 *
 * API contract:
 *   GET  /admin/info               → UpdateInfoResponse
 *   POST /admin/update             → UpdateResponse  (raw gzip body)
 *   POST /admin/update/rollback    → RollbackResponse
 *   GET  /health                   → HealthResponse  (polled after restart)
 *
 * The package MUST be signed by the operator's key; the server verifies the
 * signature before installing.  This screen replaces the running backend —
 * the server will be briefly unreachable while it restarts.
 */

import { useState, useCallback, useEffect } from 'react';
import {
  View,
  Text,
  StyleSheet,
  TouchableOpacity,
  ScrollView,
  ActivityIndicator,
  Alert,
} from 'react-native';
import * as DocumentPicker from 'expo-document-picker';
import { api, ApiError } from '../../src/api/client';
import type { UpdateInfoResponse } from '../../src/types';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type Phase =
  | 'idle'
  | 'picking'
  | 'uploading'
  | 'restarting'
  | 'done'
  | 'rolling_back'
  | 'error';

interface PickedFile {
  uri: string;
  name: string;
  size: number | undefined;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function friendlyError(err: unknown): string {
  if (err instanceof ApiError) {
    if (err.status === 401) return 'Unauthorised — check your Update token in Settings.';
    if (err.status === 400) return `Bad package: ${err.message}`;
    return err.message;
  }
  return err instanceof Error ? err.message : String(err);
}

function formatBytes(bytes: number | undefined): string {
  if (bytes == null) return '';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

// ---------------------------------------------------------------------------
// Main screen
// ---------------------------------------------------------------------------

export default function BackendUpdateScreen() {
  const [info, setInfo] = useState<UpdateInfoResponse | null>(null);
  const [infoLoading, setInfoLoading] = useState(true);
  const [infoError, setInfoError] = useState<string | null>(null);

  const [pickedFile, setPickedFile] = useState<PickedFile | null>(null);
  const [uploadProgress, setUploadProgress] = useState(0); // 0..1
  const [phase, setPhase] = useState<Phase>('idle');
  const [phaseMessage, setPhaseMessage] = useState('');
  const [newVersion, setNewVersion] = useState<string | null>(null);

  // -------------------------------------------------------------------------
  // Load /admin/info
  // -------------------------------------------------------------------------

  const loadInfo = useCallback(async () => {
    setInfoLoading(true);
    setInfoError(null);
    try {
      const data = await api.getUpdateInfo();
      setInfo(data);
    } catch (err) {
      setInfoError(friendlyError(err));
    } finally {
      setInfoLoading(false);
    }
  }, []);

  useEffect(() => {
    loadInfo();
  }, [loadInfo]);

  // -------------------------------------------------------------------------
  // Pick package file
  // -------------------------------------------------------------------------

  const pickFile = useCallback(async () => {
    setPhase('picking');
    try {
      const result = await DocumentPicker.getDocumentAsync({
        // Accept any file — operator picks the .tar.gz / .gz manually
        type: '*/*',
        copyToCacheDirectory: true,
      });

      if (result.canceled) {
        setPhase('idle');
        return;
      }

      const asset = result.assets[0];
      setPickedFile({
        uri: asset.uri,
        name: asset.name,
        size: asset.size ?? undefined,
      });
      setPhase('idle');
    } catch (err) {
      setPhase('error');
      setPhaseMessage(friendlyError(err));
    }
  }, []);

  // -------------------------------------------------------------------------
  // Upload + wait for restart
  // -------------------------------------------------------------------------

  const installUpdate = useCallback(async () => {
    if (!pickedFile) return;

    Alert.alert(
      'Install update?',
      `This will replace the running backend with "${pickedFile.name}" and trigger a restart. The server will be briefly unreachable.\n\nThe package must be signed by the operator's key.`,
      [
        { text: 'Cancel', style: 'cancel' },
        {
          text: 'Install',
          style: 'destructive',
          onPress: async () => {
            try {
              setPhase('uploading');
              setUploadProgress(0);
              setPhaseMessage('Uploading package…');

              const updateResp = await api.uploadUpdatePackage(
                pickedFile.uri,
                (fraction) => setUploadProgress(fraction),
              );

              setPhase('restarting');
              setUploadProgress(1);
              setPhaseMessage(
                `Package installed (${updateResp.from_version} → ${updateResp.to_version}). Waiting for backend to restart…`,
              );

              const health = await api.waitForHealthy(90_000);

              // Refresh info after restart
              const freshInfo = await api.getUpdateInfo().catch(() => null);
              if (freshInfo) setInfo(freshInfo);

              setNewVersion(health.version);
              setPhase('done');
              setPhaseMessage(`Backend is healthy — now running v${health.version}.`);
              setPickedFile(null);
            } catch (err) {
              setPhase('error');
              setPhaseMessage(friendlyError(err));
            }
          },
        },
      ],
    );
  }, [pickedFile]);

  // -------------------------------------------------------------------------
  // Rollback
  // -------------------------------------------------------------------------

  const rollback = useCallback(async () => {
    Alert.alert(
      'Roll back?',
      'This will restore the previous backend binary and trigger a restart.',
      [
        { text: 'Cancel', style: 'cancel' },
        {
          text: 'Roll back',
          style: 'destructive',
          onPress: async () => {
            try {
              setPhase('rolling_back');
              setPhaseMessage('Sending rollback request…');

              const rollbackResp = await api.rollbackUpdate();

              setPhaseMessage(
                `Rollback initiated (restoring ${rollbackResp.restored_version}). Waiting for backend to restart…`,
              );

              const health = await api.waitForHealthy(90_000);

              const freshInfo = await api.getUpdateInfo().catch(() => null);
              if (freshInfo) setInfo(freshInfo);

              setNewVersion(health.version);
              setPhase('done');
              setPhaseMessage(`Backend is healthy — now running v${health.version}.`);
            } catch (err) {
              setPhase('error');
              setPhaseMessage(friendlyError(err));
            }
          },
        },
      ],
    );
  }, []);

  // -------------------------------------------------------------------------
  // Reset to idle
  // -------------------------------------------------------------------------

  const reset = useCallback(() => {
    setPhase('idle');
    setPhaseMessage('');
    setUploadProgress(0);
    setNewVersion(null);
    loadInfo();
  }, [loadInfo]);

  // -------------------------------------------------------------------------
  // Render helpers
  // -------------------------------------------------------------------------

  const busy =
    phase === 'uploading' || phase === 'restarting' || phase === 'rolling_back';

  return (
    <ScrollView
      contentContainerStyle={styles.container}
      keyboardShouldPersistTaps="handled"
    >
      {/* Info card */}
      <Text style={styles.sectionHeader}>Current backend</Text>
      {infoLoading ? (
        <ActivityIndicator color="#E53935" style={styles.infoSpinner} />
      ) : infoError ? (
        <View style={styles.errorCard}>
          <Text style={styles.errorText}>{infoError}</Text>
          <TouchableOpacity style={styles.retryBtn} onPress={loadInfo}>
            <Text style={styles.retryBtnText}>Retry</Text>
          </TouchableOpacity>
        </View>
      ) : info ? (
        <View style={styles.infoCard}>
          <Row label="Version" value={info.version} />
          <Row label="Target" value={info.target} />
          <Row label="Restart mode" value={info.restart_mode} />
          <Row
            label="Updates enabled"
            value={info.update_enabled ? 'Yes' : 'No'}
            valueColor={info.update_enabled ? '#43A047' : '#E53935'}
          />
          <Row
            label="Backup available"
            value={info.has_backup ? 'Yes' : 'No'}
            valueColor={info.has_backup ? '#43A047' : '#aaa'}
          />
          <TouchableOpacity style={styles.refreshBtn} onPress={loadInfo}>
            <Text style={styles.refreshBtnText}>Refresh</Text>
          </TouchableOpacity>
        </View>
      ) : null}

      {/* Package note */}
      <View style={styles.noteCard}>
        <Text style={styles.noteText}>
          The package must be a signed .tar.gz archive produced by the operator's
          build pipeline. The server verifies the signature before installing.
          This action replaces the running backend binary.
        </Text>
      </View>

      {/* Phase: uploading / restarting / rolling back */}
      {busy && (
        <View style={styles.phaseCard}>
          <ActivityIndicator color="#E53935" style={{ marginBottom: 12 }} />
          <Text style={styles.phaseMessage}>{phaseMessage}</Text>
          {phase === 'uploading' && (
            <View style={styles.progressTrack}>
              <View
                style={[styles.progressFill, { width: `${Math.round(uploadProgress * 100)}%` }]}
              />
            </View>
          )}
          {phase === 'uploading' && (
            <Text style={styles.progressLabel}>
              {Math.round(uploadProgress * 100)}%
            </Text>
          )}
        </View>
      )}

      {/* Phase: done */}
      {phase === 'done' && (
        <View style={styles.successCard}>
          <Text style={styles.successText}>{phaseMessage}</Text>
          {newVersion && (
            <Text style={styles.newVersionLabel}>New version: {newVersion}</Text>
          )}
          <TouchableOpacity style={styles.secondaryButton} onPress={reset}>
            <Text style={styles.secondaryButtonText}>Done</Text>
          </TouchableOpacity>
        </View>
      )}

      {/* Phase: error */}
      {phase === 'error' && (
        <View style={styles.errorCard}>
          <Text style={styles.errorText}>{phaseMessage}</Text>
          <TouchableOpacity style={styles.retryBtn} onPress={reset}>
            <Text style={styles.retryBtnText}>Dismiss</Text>
          </TouchableOpacity>
        </View>
      )}

      {/* Install section (shown when idle or after file picked) */}
      {(phase === 'idle' || phase === 'picking') && (
        <>
          <Text style={[styles.sectionHeader, { marginTop: 24 }]}>Install update</Text>

          <TouchableOpacity
            style={styles.secondaryButton}
            onPress={pickFile}
            disabled={phase === 'picking'}
          >
            {phase === 'picking' ? (
              <ActivityIndicator color="#E53935" />
            ) : (
              <Text style={styles.secondaryButtonText}>Select package…</Text>
            )}
          </TouchableOpacity>

          {pickedFile && (
            <View style={styles.fileCard}>
              <Text style={styles.fileName} numberOfLines={2}>
                {pickedFile.name}
              </Text>
              {pickedFile.size != null && (
                <Text style={styles.fileSize}>{formatBytes(pickedFile.size)}</Text>
              )}
            </View>
          )}

          <TouchableOpacity
            style={[
              styles.primaryButton,
              (!pickedFile || !info?.update_enabled) && styles.primaryButtonDisabled,
            ]}
            onPress={installUpdate}
            disabled={!pickedFile || !info?.update_enabled}
            accessibilityRole="button"
          >
            <Text style={styles.primaryButtonText}>Install update</Text>
          </TouchableOpacity>

          {info && !info.update_enabled && (
            <Text style={styles.disabledHint}>
              Updates are disabled on this server (update_enabled: false).
            </Text>
          )}

          {/* Rollback */}
          {info?.has_backup && (
            <>
              <Text style={[styles.sectionHeader, { marginTop: 32 }]}>Rollback</Text>
              <TouchableOpacity
                style={styles.rollbackButton}
                onPress={rollback}
                accessibilityRole="button"
              >
                <Text style={styles.rollbackButtonText}>Roll back to previous version</Text>
              </TouchableOpacity>
              <Text style={styles.hint}>
                Restores the backup binary created before the last update and restarts the server.
              </Text>
            </>
          )}
        </>
      )}
    </ScrollView>
  );
}

// ---------------------------------------------------------------------------
// Sub-component
// ---------------------------------------------------------------------------

function Row({
  label,
  value,
  valueColor,
}: {
  label: string;
  value: string;
  valueColor?: string;
}) {
  return (
    <View style={styles.row}>
      <Text style={styles.rowLabel}>{label}</Text>
      <Text style={[styles.rowValue, valueColor ? { color: valueColor } : undefined]}>
        {value}
      </Text>
    </View>
  );
}

// ---------------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------------

const styles = StyleSheet.create({
  container: {
    padding: 20,
    paddingBottom: 48,
    backgroundColor: '#fff',
  },
  sectionHeader: {
    fontSize: 11,
    fontWeight: '700',
    color: '#aaa',
    textTransform: 'uppercase',
    letterSpacing: 1,
    marginBottom: 12,
  },
  infoSpinner: {
    marginVertical: 16,
  },
  infoCard: {
    borderWidth: 1,
    borderColor: '#eee',
    borderRadius: 10,
    padding: 14,
    marginBottom: 16,
    backgroundColor: '#fafafa',
  },
  row: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    paddingVertical: 5,
  },
  rowLabel: {
    fontSize: 13,
    color: '#666',
  },
  rowValue: {
    fontSize: 13,
    fontWeight: '500',
    color: '#111',
    flexShrink: 1,
    textAlign: 'right',
    marginLeft: 12,
  },
  refreshBtn: {
    marginTop: 10,
    alignSelf: 'flex-end',
  },
  refreshBtnText: {
    fontSize: 13,
    color: '#E53935',
    fontWeight: '600',
  },
  noteCard: {
    backgroundColor: '#FFF8E1',
    borderRadius: 8,
    padding: 12,
    marginBottom: 16,
  },
  noteText: {
    fontSize: 12,
    color: '#795548',
    lineHeight: 18,
  },
  phaseCard: {
    borderWidth: 1,
    borderColor: '#eee',
    borderRadius: 10,
    padding: 16,
    marginBottom: 16,
    alignItems: 'center',
    backgroundColor: '#fafafa',
  },
  phaseMessage: {
    fontSize: 14,
    color: '#333',
    textAlign: 'center',
    lineHeight: 20,
    marginBottom: 10,
  },
  progressTrack: {
    width: '100%',
    height: 6,
    borderRadius: 3,
    backgroundColor: '#eee',
    overflow: 'hidden',
    marginBottom: 6,
  },
  progressFill: {
    height: '100%',
    backgroundColor: '#E53935',
    borderRadius: 3,
  },
  progressLabel: {
    fontSize: 12,
    color: '#888',
    fontVariant: ['tabular-nums'],
  },
  successCard: {
    borderWidth: 1,
    borderColor: '#C8E6C9',
    borderRadius: 10,
    padding: 16,
    marginBottom: 16,
    backgroundColor: '#F1F8E9',
    alignItems: 'center',
  },
  successText: {
    fontSize: 14,
    color: '#2E7D32',
    textAlign: 'center',
    lineHeight: 20,
    marginBottom: 8,
  },
  newVersionLabel: {
    fontSize: 13,
    color: '#43A047',
    fontWeight: '600',
    marginBottom: 12,
  },
  errorCard: {
    borderWidth: 1,
    borderColor: '#FFCDD2',
    borderRadius: 10,
    padding: 16,
    marginBottom: 16,
    backgroundColor: '#FFF3F3',
    alignItems: 'center',
  },
  errorText: {
    fontSize: 14,
    color: '#C62828',
    textAlign: 'center',
    lineHeight: 20,
    marginBottom: 10,
  },
  retryBtn: {
    borderWidth: 1,
    borderColor: '#E53935',
    borderRadius: 6,
    paddingHorizontal: 16,
    paddingVertical: 6,
  },
  retryBtnText: {
    color: '#E53935',
    fontWeight: '600',
    fontSize: 13,
  },
  secondaryButton: {
    borderWidth: 1,
    borderColor: '#E53935',
    borderRadius: 8,
    padding: 10,
    alignItems: 'center',
    marginBottom: 12,
  },
  secondaryButtonText: {
    color: '#E53935',
    fontWeight: '600',
    fontSize: 14,
  },
  fileCard: {
    borderWidth: 1,
    borderColor: '#ddd',
    borderRadius: 8,
    padding: 12,
    marginBottom: 12,
    backgroundColor: '#fafafa',
  },
  fileName: {
    fontSize: 14,
    color: '#111',
    fontWeight: '500',
  },
  fileSize: {
    fontSize: 12,
    color: '#888',
    marginTop: 4,
  },
  primaryButton: {
    backgroundColor: '#E53935',
    borderRadius: 10,
    padding: 14,
    alignItems: 'center',
    marginBottom: 6,
  },
  primaryButtonDisabled: {
    opacity: 0.4,
  },
  primaryButtonText: {
    color: '#fff',
    fontSize: 16,
    fontWeight: '700',
  },
  disabledHint: {
    fontSize: 12,
    color: '#E53935',
    textAlign: 'center',
    marginBottom: 8,
  },
  rollbackButton: {
    borderWidth: 1.5,
    borderColor: '#FB8C00',
    borderRadius: 8,
    padding: 10,
    alignItems: 'center',
    marginBottom: 8,
  },
  rollbackButtonText: {
    color: '#FB8C00',
    fontWeight: '600',
    fontSize: 14,
  },
  hint: {
    fontSize: 12,
    color: '#bbb',
    lineHeight: 17,
    marginBottom: 8,
  },
});
