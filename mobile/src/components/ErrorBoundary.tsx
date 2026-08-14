/**
 * App-level error boundary.  Catches JS render/runtime errors anywhere in the
 * tree and shows a Kiln-styled fallback with the message — so a crash gives the
 * user (and us) feedback instead of a silently vanished session.  Native crashes
 * (Skia / audio codec) can't be caught here; those need the device logcat.
 */

import { Component, type ReactNode } from 'react';
import { View, Text, TouchableOpacity, StyleSheet, ScrollView } from 'react-native';
import { colors, mono } from '../theme';

interface Props {
  children: ReactNode;
}
interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: { componentStack?: string }) {
    // Surfaced in Metro / device logs for diagnosis.
    console.error('[Scribe] Uncaught error:', error, info?.componentStack);
  }

  reset = () => this.setState({ error: null });

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    return (
      <View style={styles.root}>
        <ScrollView contentContainerStyle={styles.content}>
          <Text style={styles.badge}>! SOMETHING BROKE</Text>
          <Text style={styles.title}>The app hit an error</Text>
          <Text style={styles.message}>{error.message || String(error)}</Text>
          {error.stack ? <Text style={styles.stack}>{error.stack}</Text> : null}
          <TouchableOpacity style={styles.button} onPress={this.reset}>
            <Text style={styles.buttonText}>Try again</Text>
          </TouchableOpacity>
        </ScrollView>
      </View>
    );
  }
}

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: colors.bg },
  content: { flexGrow: 1, justifyContent: 'center', padding: 28, gap: 12 },
  badge: {
    fontSize: 11,
    fontWeight: '700',
    color: colors.accentDeep,
    fontFamily: mono,
    letterSpacing: 1.5,
  },
  title: { fontSize: 22, fontWeight: '700', color: colors.textPrimary },
  message: { fontSize: 14, lineHeight: 20, color: colors.textLight },
  stack: {
    fontSize: 10,
    lineHeight: 15,
    color: colors.textDim,
    fontFamily: mono,
    marginTop: 4,
  },
  button: {
    marginTop: 16,
    alignSelf: 'flex-start',
    backgroundColor: colors.accent,
    borderRadius: 12,
    paddingHorizontal: 20,
    paddingVertical: 12,
  },
  buttonText: { color: colors.white, fontWeight: '700', fontSize: 15 },
});
