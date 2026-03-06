/* test/integration/pkg/term/tree.go */

package term

import (
	"fmt"
	"strings"
)

type TreeNode struct {
	Label    string
	Children []*TreeNode
}

func NewNode(label string) *TreeNode {
	return &TreeNode{Label: label, Children: []*TreeNode{}}
}

func (n *TreeNode) Add(label string) *TreeNode {
	child := NewNode(label)
	n.Children = append(n.Children, child)
	return child
}

func (n *TreeNode) String() string {
	return n.StringWithPrefix("")
}

// StringWithPrefix renders the tree with a custom indentation prefix
func (n *TreeNode) StringWithPrefix(indent string) string {
	var sb strings.Builder
	if n.Label != "" {
		sb.WriteString(indent + n.Label + "\n")
	}
	renderTree(&sb, n.Children, indent)
	return strings.TrimRight(sb.String(), "\n")
}

func renderTree(sb *strings.Builder, nodes []*TreeNode, prefix string) {
	for i, node := range nodes {
		isLast := i == len(nodes)-1
		connector := "├─ "
		childPrefix := "│  "
		if isLast {
			connector = "└─ "
			childPrefix = "   "
		}

		sb.WriteString(prefix + connector + node.Label + "\n")
		renderTree(sb, node.Children, prefix+childPrefix)
	}
}

// FormatFailure formats the error message and the tree.
// It aligns the tree structure under the "Details" text.
func FormatFailure(reason string, details *TreeNode) error {
	if details == nil {
		return fmt.Errorf("  └─ Details: %s", reason)
	}
	// "  └─ Details: " is roughly indented. We want the tree to align nicely below it.
	// We add a specific indentation for the tree root.
	treeOutput := details.StringWithPrefix("     ")
	return fmt.Errorf("  └─ Details: %s\n%s", reason, treeOutput)
}
