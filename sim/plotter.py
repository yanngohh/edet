import matplotlib.pyplot as plt
import pandas as pd
import os

def plot_scenario(csv_path, title, x_col, line_configs, show=True, save_path=None, y_label="Value", seed=None):
    """
    Plots simulation results using Matplotlib.
    line_configs: list of dicts {'col': str, 'label': str, 'color': str, 'ls': str}
    """
    if not os.path.exists(csv_path):
        print(f"Error: {csv_path} not found.")
        return

    df = pd.read_csv(csv_path)
    if df.empty:
        print(f"Error: No data in {csv_path}")
        return

    fig = plt.figure(figsize=(10, 6))
    
    # Window Title
    win_title = title
    if seed is not None:
        win_title = f"[Scenario #{seed}] {title}"
    
    try:
        fig.canvas.manager.set_window_title(win_title)
    except:
        pass
        
    plt.title(win_title, fontsize=14, fontweight='bold')
    
    for cfg in line_configs:
        plt.plot(df[x_col], df[cfg['col']], 
                 label=cfg['label'], 
                 color=cfg.get('color'), 
                 linestyle=cfg.get('ls', '-'), 
                 linewidth=2)

    plt.xlabel(x_col.capitalize())
    plt.ylabel(y_label)
    plt.grid(True, linestyle='--', alpha=0.7)
    plt.legend(loc='best')
    
    if save_path:
        # Save as PNG as well, as it's often more reliable for previews
        png_path = save_path.replace('.svg', '.png')
        plt.savefig(png_path, dpi=150)
        plt.savefig(save_path)
        print(f"Plot saved to {save_path} and {png_path}")

    if show:
        print(f"Displaying plot for '{title}'...")
        plt.show()
    else:
        plt.close()

def plot_mixed_scenario(csv_path, show=True, save_path=None, seed=None, annotations=None):
    if not os.path.exists(csv_path): return
    df = pd.read_csv(csv_path)
    
    fig = plt.figure(figsize=(12, 7))
    
    win_title = "Mixed Attack Scenario & System Response"
    if seed is not None:
        win_title = f"[Scenario #{seed}] {win_title}"
        
    try:
        fig.canvas.manager.set_window_title(win_title)
    except:
        pass
        
    plt.title(win_title, fontsize=16, fontweight='bold')
    
    plt.plot(df['epoch'], df['honest_avg_trust'], label='Honest Trust', color='green', linewidth=2.5)
    plt.plot(df['epoch'], df['attacker_trust'], label='Malicious Trust Peak', color='red', linewidth=3)
    plt.plot(df['epoch'], df['sybil_avg_trust'], label='Sybil Ring Trust', color='purple', linestyle='--', alpha=0.8)
    
    plt.plot(df['epoch'], df['honest_avg_pressure'], label='Honest Pressure', color='blue', linestyle='--', alpha=0.4)
    plt.plot(df['epoch'], df['attacker_pressure'], label='Malicious Pressure Min', color='orange', linestyle=':', linewidth=2)
    plt.plot(df['epoch'], df['sybil_avg_pressure'], label='Sybil Ring Pressure', color='brown', linestyle=':', alpha=0.6)
    
    # Dynamic Annotations
    if annotations:
        for ann in annotations:
            plt.axvline(x=ann['x'], color='gray', linestyle='--', alpha=0.5)
            plt.text(ann['x'] + 1, plt.ylim()[1]*0.9, ann['label'], color='gray', fontstyle='italic')
    
    plt.xlabel("Epoch")
    plt.ylabel("Score / Indicator")
    plt.grid(True, alpha=0.3)
    plt.legend()
    
    if save_path:
        plt.savefig(save_path.replace('.svg', '.png'), dpi=150)
        plt.savefig(save_path)
    
    if show:
        plt.show()
    else:
        plt.close()

def plot_gateway_scenario(csv_path, show=True, save_path=None, seed=None):
    """Plot gateway attack with pressure, trust, and debt metrics."""
    if not os.path.exists(csv_path):
        print(f"Error: {csv_path} not found.")
        return
    
    df = pd.read_csv(csv_path)
    if df.empty:
        print(f"Error: No data in {csv_path}")
        return
    
    fig, (ax1, ax2, ax3) = plt.subplots(3, 1, figsize=(12, 10), sharex=True)
    
    win_title = "Gateway Attack Analysis"
    if seed is not None:
        win_title = f"[Scenario #{seed}] {win_title}"
    
    try:
        fig.canvas.manager.set_window_title(win_title)
    except:
        pass
    
    fig.suptitle(win_title, fontsize=16, fontweight='bold')
    
    # Add attack phase annotations to all panels
    for ax in [ax1, ax2, ax3]:
        # Phase 1: Reputation Building (epochs 0-20)
        ax.axvspan(0, 20, alpha=0.1, color='green', label='Phase 1: Reputation Building' if ax == ax1 else '')
        # Phase 2: Attack (epochs 21-40)
        ax.axvspan(21, 40, alpha=0.15, color='red', label='Phase 2: Attack' if ax == ax1 else '')
        # Phase boundaries
        ax.axvline(x=20, color='gray', linestyle='--', alpha=0.5, linewidth=1.5)
        ax.axvline(x=40, color='gray', linestyle='--', alpha=0.5, linewidth=1.5)
    
    # Panel 1: Capacity (used as pressure proxy: lower capacity → higher pressure)
    # gateway_pressure and victim_pressure are derived from capacity utilization if
    # the raw pressure columns are absent (gateway suite writes capacity instead).
    if 'gateway_pressure' in df.columns:
        gw_pressure_col = 'gateway_pressure'
        vic_pressure_col = 'victim_pressure'
    else:
        # Derive pressure from capacity: pressure = debt / capacity (0 = no stress, 1 = maxed out).
        # If debt columns are present, compute; otherwise fall back to capacity directly.
        if 'gateway_debt' in df.columns and 'gateway_capacity' in df.columns:
            df['gateway_pressure'] = df['gateway_debt'] / df['gateway_capacity'].replace(0, float('nan'))
            df['victim_pressure'] = df['victim_debt'] / df['victim_capacity'].replace(0, float('nan'))
        else:
            df['gateway_pressure'] = df.get('gateway_capacity', 0.0)
            df['victim_pressure'] = df.get('victim_capacity', 0.0)
        gw_pressure_col = 'gateway_pressure'
        vic_pressure_col = 'victim_pressure'
    ax1.plot(df['epoch'], df[gw_pressure_col], label='Gateway Pressure', color='red', linewidth=2.5, marker='o', markersize=3, markevery=5)
    ax1.plot(df['epoch'], df[vic_pressure_col], label='Victim Pressure', color='green', linewidth=2.5)
    ax1.set_ylabel('Pressure (π)', fontsize=12)
    ax1.grid(True, alpha=0.3)
    ax1.legend(loc='best')
    ax1.set_title('Pressure Indicators', fontsize=12, fontweight='bold')
    # Annotate minimum gateway pressure
    min_gw_pressure_idx = df[gw_pressure_col].idxmin()
    min_gw_pressure = df.loc[min_gw_pressure_idx, gw_pressure_col]
    min_gw_epoch = df.loc[min_gw_pressure_idx, 'epoch']
    ax1.annotate(f'Min: {min_gw_pressure:.3f}', 
                 xy=(min_gw_epoch, min_gw_pressure), 
                 xytext=(min_gw_epoch+5, min_gw_pressure-0.05),
                 arrowprops=dict(arrowstyle='->', color='red', lw=1.5),
                 fontsize=10, color='red', fontweight='bold')
    
    # Panel 2: Trust
    ax2.plot(df['epoch'], df['gateway_trust'], label='Gateway Trust', color='darkred', linewidth=2.5, marker='s', markersize=3, markevery=5)
    ax2.plot(df['epoch'], df['victim_trust'], label='Victim Trust', color='darkgreen', linewidth=2.5)
    ax2.set_ylabel('Trust (τ)', fontsize=12)
    ax2.grid(True, alpha=0.3)
    ax2.legend(loc='best')
    ax2.set_title('Trust Scores', fontsize=12, fontweight='bold')
    # Annotate minimum gateway trust
    min_gw_trust_idx = df['gateway_trust'].idxmin()
    min_gw_trust = df.loc[min_gw_trust_idx, 'gateway_trust']
    min_trust_epoch = df.loc[min_gw_trust_idx, 'epoch']
    ax2.annotate(f'Min: {min_gw_trust:.3f}', 
                 xy=(min_trust_epoch, min_gw_trust), 
                 xytext=(min_trust_epoch+5, min_gw_trust+0.1),
                 arrowprops=dict(arrowstyle='->', color='darkred', lw=1.5),
                 fontsize=10, color='darkred', fontweight='bold')
    
    # Panel 3: Debt
    ax3.plot(df['epoch'], df['gateway_debt'], label='Gateway Debt', color='orange', linewidth=2.5, marker='^', markersize=3, markevery=5)
    ax3.plot(df['epoch'], df['victim_debt'], label='Victim Debt', color='blue', linewidth=2.5)
    ax3.set_ylabel('Debt (δ)', fontsize=12)
    ax3.set_xlabel('Epoch', fontsize=12)
    ax3.grid(True, alpha=0.3)
    ax3.legend(loc='best')
    ax3.set_title('Outstanding Debt', fontsize=12, fontweight='bold')
    # Annotate maximum gateway debt
    max_gw_debt_idx = df['gateway_debt'].idxmax()
    max_gw_debt = df.loc[max_gw_debt_idx, 'gateway_debt']
    max_debt_epoch = df.loc[max_gw_debt_idx, 'epoch']
    if max_gw_debt > 0:
        ax3.annotate(f'Max: {max_gw_debt:.1f}', 
                     xy=(max_debt_epoch, max_gw_debt), 
                     xytext=(max_debt_epoch+5, max_gw_debt*0.8),
                     arrowprops=dict(arrowstyle='->', color='orange', lw=1.5),
                     fontsize=10, color='orange', fontweight='bold')
    
    plt.tight_layout()
    
    if save_path:
        plt.savefig(save_path.replace('.svg', '.png'), dpi=150)
        plt.savefig(save_path)
        print(f"Plot saved to {save_path}")
    
    if show:
        print(f"Displaying gateway attack analysis...")
        plt.show()
    else:
        plt.close()
