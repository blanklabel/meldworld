package main

import (
	"github.com/blanklabel/meldworld/model"
)

// NewLIFOQueue returns a new LIFOQueue.
func NewLIFOQueue() *LIFOQueue {
	return &LIFOQueue{}
}

// Stack is a basic LIFO stack that resizes as needed.
type LIFOQueue struct {
	items []*model.EntityAction
	count int
}

// Push adds an Item to the queue.
func (s *LIFOQueue) Push(n *model.EntityAction) {
	s.items = append(s.items[:s.count], n)
	s.count++
}

// Pop removes and returns a node from the stack in last to first order.
func (s *LIFOQueue) Pop() *model.EntityAction {
	if s.count == 0 {
		return nil
	}
	s.count--
	return s.items[s.count]
}

// Push adds an Item to the queue.
func (s *LIFOQueue) GetSize() int {
	return s.count
}
