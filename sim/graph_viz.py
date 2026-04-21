import os
import sys
import networkx as nx
from .universe import HAS_GPU
from .config import RESULTS_DIR

# Configure matplotlib backend BEFORE importing pyplot
# This ensures compatibility with Wayland, X11, and headless environments
import matplotlib

def select_backend():
    """Select the appropriate matplotlib backend based on environment."""
    # Check if running in headless/save mode
    if os.environ.get('DISPLAY') is None or '--no-display' in sys.argv:
        return 'Agg'  # Non-interactive backend for headless/save mode
    
    # Check if running on Wayland
    session_type = os.environ.get('XDG_SESSION_TYPE', '').lower()
    wayland_display = os.environ.get('WAYLAND_DISPLAY')
    
    if session_type == 'wayland' or wayland_display:
        # Try GTK3 backend which works natively with Wayland
        try:
            import gi
            gi.require_version('Gtk', '3.0')
            return 'GTK3Agg'
        except (ImportError, ValueError):
            # GTK3 not available, try Qt backends which also support Wayland
            try:
                import PyQt6
                return 'QtAgg'  # Qt6 backend
            except ImportError:
                try:
                    import PyQt5
                    return 'Qt5Agg'  # Qt5 backend
                except ImportError:
                    # Fall back to Agg if no Wayland-compatible backend available
                    print("[WARN] Wayland detected but no compatible GUI backend (GTK3/Qt5/Qt6) found.")
                    print("[WARN] Falling back to non-interactive mode. Install python3-gi, PyQt5, or PyQt6 for interactive plotting.")
                    return 'Agg'
    
    # Default to TkAgg for X11 sessions
    return 'TkAgg'

matplotlib.use(select_backend())

import matplotlib.pyplot as plt
from matplotlib.animation import FuncAnimation
import numpy as np

class GraphVisualizer:
    def __init__(self, universe, node_limit=200, seed=None):
        self.universe = universe
        self.node_limit = node_limit
        self.G = nx.DiGraph()
        self.pos = None
        
        # Use GridSpec for more robust sub-plots
        self.fig = plt.figure(figsize=(14, 8))
        
        # Window Title
        gpu_str = "[GPU]" if HAS_GPU else "[CPU]"
        win_title = f"{gpu_str} edet Network Topology"
        if seed is not None:
             win_title = f"[Scenario #{seed}] {win_title}"
        
        try:
            self.fig.canvas.manager.set_window_title(win_title)
        except:
            pass
            
        gs = self.fig.add_gridspec(1, 2, width_ratios=[3.5, 1], wspace=0.1)
        self.ax = self.fig.add_subplot(gs[0])
        self.legend_ax = self.fig.add_subplot(gs[1])
        self.legend_ax.axis('off')
        
        # Event log state
        self.event_log = []
        self.event_text_obj = None
        self.involved_nodes = set()
        self.COLOR_ATTACKER = '#0066FF' # Blue
        self.COLOR_ACTIVE = '#000000'   # Black
        self.COLOR_NORMAL = '#555555'   # Dark Gray
        
        # Track active nodes (peers that joined or are core)
        self.active_nodes = set()
        
        # Interactive state
        self.paused = True
        self.step_requested = False
        self.fig.canvas.mpl_connect('key_press_event', self._on_key)
        self.fig.canvas.mpl_connect('button_press_event', self._on_click)
        
        print("\n--- Interactive Viz Controls ---")
        print("SPACE or CLICK : Single Step")
        print("P              : Toggle Auto-Play")
        print("------------------------------\n")

    def _draw_legend(self):
        self.legend_ax.clear()
        self.legend_ax.axis('off')
        # Explicit limits to ensure text at 0.1, 0.25, etc. is visible
        self.legend_ax.set_xlim(0, 1)
        self.legend_ax.set_ylim(0, 1)
        self.legend_ax.set_title("QUANTITATIVE LEGEND", fontsize=10, fontweight='bold')

        # 1. NODE METRICS (Top Row)
        self.legend_ax.text(0, 0.94, "NODE METRICS", fontweight='bold', fontsize=8)
        
        # Relative Trust (Color)
        self.legend_ax.text(0.05, 0.88, "Trust:", fontsize=8)
        colors = [(1,0.2,0.2), (0.5,0.5,0.5), (0.2,1,0.2)] # Red, Grey, Green
        for i, c in enumerate(colors):
             self.legend_ax.scatter(0.25 + i*0.1, 0.89, color=c, s=30, edgecolors='#333')
        self.legend_ax.text(0.55, 0.88, "Low  \u2192  High", fontsize=7)

        # Capacity Size (Qualitative)
        self.legend_ax.text(0.05, 0.81, "Cap:", fontsize=8)
        for i, s in enumerate([20, 50, 100]):
             self.legend_ax.scatter(0.25 + i*0.12, 0.82, color='#888', s=s/4, alpha=0.5)
        self.legend_ax.text(0.60, 0.81, "Less  \u2192  More", fontsize=7)

        # Debt Border (Relative to Capacity)
        self.legend_ax.text(0.05, 0.74, "Debt:", fontsize=8)
        for i, (w, label) in enumerate([(1.0, "0%"), (5.5, "50%"), (10.0, "100%")]):
             self.legend_ax.scatter(0.25 + i*0.18, 0.75, facecolor='none', edgecolors='#333', s=40, linewidths=w)
             self.legend_ax.text(0.25 + i*0.18 - 0.02, 0.70, label, fontsize=6)

        # 2. FLOW STYLES (Bottom Row)
        self.legend_ax.text(0, 0.62, "FLOW STYLES", fontweight='bold', fontsize=8)
        
        # Trade / Attack
        self.legend_ax.plot([0.05, 0.2], [0.55, 0.55], color=self.COLOR_ACTIVE, linewidth=1.5)
        self.legend_ax.text(0.25, 0.54, "Trade", fontsize=8)
        
        self.legend_ax.plot([0.05, 0.2], [0.47, 0.47], color=self.COLOR_ATTACKER, linewidth=1.5)
        self.legend_ax.text(0.25, 0.46, "Attack", fontsize=8)

    def _on_key(self, event):
        if event.key == ' ':
            self.step_requested = True
        elif event.key == 'p':
            self.paused = not self.paused
            print(f"[UI] Paused: {self.paused}")

    def _on_click(self, event):
        self.step_requested = True

    def set_frame_context(self, events, involved_nodes):
        if events:
            timestamped = [f"[E:{self.universe.epoch:02d}] {e}" for e in events]
            self.event_log.extend(timestamped)
        else:
            # Just a tick to show time is moving and scroll old events
            self.event_log.append(f"[E:{self.universe.epoch:02d}]  ")
            
        self.event_log = self.event_log[-5:]
        self.involved_nodes = involved_nodes if involved_nodes else set()

    def _update_event_display(self):
        if not self.event_log:
            return
            
        text = "\n".join(self.event_log)
        attackers = self._get_attacker_nodes()
        is_attack = any(n in attackers for n in self.involved_nodes)
        color = self.COLOR_ATTACKER if is_attack else self.COLOR_ACTIVE
        
        if self.event_text_obj:
            self.event_text_obj.set_text(text)
            self.event_text_obj.set_color(color)
        else:
            self.event_text_obj = self.ax.text(0.02, 0.02, text, transform=self.ax.transAxes, 
                                               verticalalignment='bottom', fontsize=10, 
                                               family='monospace', fontweight='bold',
                                               color=color,
                                               bbox=dict(facecolor='white', alpha=0.7, edgecolor='none'))

    def _get_node_color(self, node_idx):
        t = self.universe.global_trust[node_idx]
        rel_trust = t * self.universe.size
        val = min(rel_trust / 2.0, 1.0) # 0..1
        return (1.0 - val, val, 0.0)

    def _get_node_size(self, node_idx):
        cap = self.universe.credit_capacity[node_idx]
        # In subjective mode, capacity is often ~5000+ for average nodes
        ratio = cap / 5000.0
        return 50 + ratio * 250

    def _get_node_border_thickness(self, node_idx):
        total_debt = sum(c.amount for c in self.universe.contracts[node_idx])
        cap = self.universe.credit_capacity[node_idx]
        # Ratio scaled to 1.0 (baseline) up to 10.0 (max capacity)
        stress = min(total_debt / cap, 1.2) if cap > 0 else 0.0
        return 1.0 + (stress * 7.5)

    def _get_attacker_nodes(self):
        attackers = set()
        s = self.universe.suite_state
        if 'gateway_roles' in s:
            r = s['gateway_roles']
            attackers.add(r['gateway'])
            attackers.update(r['accomplices'])
        if 'mixed_roles' in s:
            r = s['mixed_roles']
            if 'attacker_pool' in r:
                attackers.update(r['attacker_pool'])
            elif 'attacker' in r:
                attackers.add(r['attacker'])
                attackers.update(r['sybil_ring'])
        if 'sybil_roles' in s:
            r = s['sybil_roles']
            for i in range(r['sybil_count']):
                attackers.add(r['sybil_start'] + i)
        if 'slacker_roles' in s:
            attackers.add(s['slacker_roles']['slacker'])
        if 'oscillation_roles' in s:
            attackers.add(s['oscillation_roles']['attacker'])
        if 'whitewashing_roles' in s:
            attackers.add(s['whitewashing_roles']['repayer'])
            attackers.add(s['whitewashing_roles']['whitewasher'])
            attackers.add(s['whitewashing_roles']['new_identity'])
        return attackers

    def update_topology(self):
        hidden = self.universe.suite_state.get('hidden_nodes', set())
        for i in range(min(self.universe.size, self.node_limit)):
            if i in hidden:
                if i in self.active_nodes:
                    self.G.remove_node(i)
                    self.active_nodes.remove(i)
                continue

            if i not in self.active_nodes:
                self.G.add_node(i)
                self.active_nodes.add(i)
            
            self.G.nodes[i]['color'] = self._get_node_color(i)
            self.G.nodes[i]['size'] = self._get_node_size(i)
            self.G.nodes[i]['border'] = self._get_node_border_thickness(i)
            
        current_valid_edges = set()
        for i in range(min(self.universe.size, self.node_limit)):
            t_i = self.universe.global_trust[i] * self.universe.size
            for neighbor in self.universe.local_trust[i]:
                if neighbor < self.node_limit:
                    t_n = self.universe.global_trust[neighbor] * self.universe.size
                    weight = 1.0 - min(abs(t_i - t_n), 1.0)
                    
                    if not self.G.has_edge(i, neighbor):
                        self.G.add_edge(i, neighbor, weight=weight)
                    else:
                        self.G.edges[i, neighbor]['weight'] = weight
                    current_valid_edges.add((i, neighbor))
        
        to_remove = [(u, v) for u, v in self.G.edges() if (u, v) not in current_valid_edges]
        self.G.remove_edges_from(to_remove)

    def draw_frame(self, frame_idx):
        self.ax.clear()
        self.event_text_obj = None 
        self.update_topology()
        
        if self.pos is None:
            self.pos = nx.spring_layout(self.G, k=0.8, weight='weight', iterations=50)
        else:
            self.pos = nx.spring_layout(self.G, pos=self.pos, k=0.8, weight='weight', iterations=5)

        nodes = list(self.G.nodes())
        node_colors = [self.G.nodes[n]['color'] for n in nodes]
        node_sizes = [self.G.nodes[n]['size'] for n in nodes]
        
        edgecolors = []
        linewidths = []
        attackers = self._get_attacker_nodes()
        
        for n in nodes:
            border_base = self.G.nodes[n].get('border', 1.0)
            if n in attackers:
                edgecolors.append(self.COLOR_ATTACKER)
                linewidths.append(border_base)
            elif n in self.involved_nodes:
                edgecolors.append(self.COLOR_ACTIVE)
                linewidths.append(border_base)
            else:
                edgecolors.append(self.COLOR_NORMAL)
                linewidths.append(border_base)

        nx.draw_networkx_nodes(self.G, self.pos, ax=self.ax, 
                               nodelist=nodes, 
                               node_color=node_colors, 
                               node_size=node_sizes,
                               linewidths=linewidths,
                               alpha=0.8, edgecolors=edgecolors)
        
        # 2. Draw Edges
        # Only draw active successful transaction arrows
        current_edges = []
        for s, b, _amount, is_attack, is_prop in self.universe.last_epoch_tx:
            if s < self.node_limit and b < self.node_limit:
                current_edges.append((s, b, is_attack, is_prop))
        
        if current_edges:
            by_target = {}
            for u, v, is_attack, is_prop in current_edges:
                by_target.setdefault(u, []).append((v, u, is_attack, is_prop))

            for target, edges in by_target.items():
                target_size = self.G.nodes[target]['size']
                radius = np.sqrt(target_size / np.pi) 

                edges_to_draw = []
                edge_colors = []
                for u, v, is_attack, _ in edges:
                     color = self.COLOR_ATTACKER if is_attack else self.COLOR_ACTIVE
                     edges_to_draw.append((u, v))
                     edge_colors.append(color)

                if edges_to_draw:
                    nx.draw_networkx_edges(self.G, self.pos, ax=self.ax, 
                                           edgelist=edges_to_draw,
                                           edge_color=edge_colors, 
                                           width=1.5,
                                           alpha=1.0, 
                                           arrows=True,
                                           arrowsize=15,
                                           min_target_margin=radius,
                                           connectionstyle="arc3")
        
        labels = {n: str(n) for n in nodes}
        nx.draw_networkx_labels(self.G, self.pos, labels=labels, ax=self.ax, font_size=8)
        gpu_str = " (GPU)" if HAS_GPU else " (CPU)"
        self.ax.set_title(f"edet Network Topology - Epoch {self.universe.epoch}{gpu_str}", fontsize=12, fontweight='bold')
        self.ax.axis('off')
        self._update_event_display()
        self._draw_legend()

    def show(self, interval=500):
        plt.show()

def start_animation(universe, update_fn, frames=100, interval=200, seed=None, save_path=None):
    print(f"[INFO] Launching GraphVisualizer v0.1.2 (Legend + Mesh support)")
    
    import os
    is_headless = os.environ.get('DISPLAY') is None
    if is_headless and save_path is None:
        os.makedirs(RESULTS_DIR, exist_ok=True)
        save_path = os.path.join(RESULTS_DIR, "animation.mp4")

    viz = GraphVisualizer(universe, seed=seed)
    res = update_fn(universe, universe.epoch)
    events, involved = res if isinstance(res, tuple) else (res, set())
    viz.set_frame_context(events, involved)
    viz.draw_frame(universe.epoch)
    universe.tick()
    
    def animate(i):
        if (save_path is not None) or (not viz.paused or viz.step_requested):
            res = update_fn(universe, universe.epoch)
            events, involved = res if isinstance(res, tuple) else (res, set())
            viz.set_frame_context(events, involved)
            viz.draw_frame(universe.epoch)
            universe.tick()
            viz.step_requested = False
            if i % 10 == 0:
                print(f"  Rendering frame {i}/{frames}...", end='\r', flush=True)
            
    ani = FuncAnimation(viz.fig, animate, frames=frames, interval=50, repeat=True)
    if save_path:
        ani.save(save_path, writer='ffmpeg', fps=20, dpi=100, extra_args=['-vcodec', 'libx264'])
    else:
        plt.show()
    return ani
